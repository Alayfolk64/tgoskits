//! Board-hosted browser transports for the Axvisor and guest consoles.

use core::sync::atomic::{AtomicBool, Ordering};
use std::{
    string::String,
    sync::{Arc, LazyLock, Mutex},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use axvm::{AxVmError, VMId};
use {
    ax_std::os::arceos::api::task::AxCpuMask,
    ax_std::os::arceos::api::task::ax_set_current_affinity,
    ax_std::os::arceos::modules::ax_runtime::task::sync::irq::IrqWaitCell,
    ax_std::os::arceos::modules::ax_runtime::task::sync::irq::IrqWorkerWaiter,
    ax_std::os::arceos::modules::ax_runtime::task::thread::current::current_thread_handle,
};

mod layout;

mod delivery;
mod lane;

use crate::sync::MutexExt;

use delivery::{DeliveryFrame, DeliveryQueue};
use lane::{NetworkOutputBatch, NetworkOutputLane};
pub(crate) use layout::LaneAllocation;
use layout::{ConsoleLane, Endpoint, Layout, LayoutFull, MAX_GUEST_CONSOLES};

const CONSOLE_LANE_COUNT: usize = ConsoleLane::COUNT;
const OUTPUT_FRAME_TARGET_CAPACITY: usize = 4096;
const OUTPUT_DELIVERY_QUEUE_CAPACITY: usize = 64 * 1024;
const OUTPUT_DELIVERY_BATCH_CAPACITY: usize = 4096;
const OUTPUT_COALESCE_WINDOW: Duration = Duration::from_millis(10);
const MANAGEMENT_LINE_CAPACITY: usize = 256;
pub(crate) const MANAGEMENT_CPU_ID: usize = 0;
/// CPU the browser-console output dispatchers run on, or `None` to leave the
/// scheduler free to place them.
///
/// The default is [`MANAGEMENT_CPU_ID`], the core the console output task
/// already runs on, because a dispatcher and that task are producer and consumer
/// of the same frame: the dispatcher fills the delivery queue and notifies, the
/// output task drains it into the socket. Sharing a core makes that handoff a
/// same-core wakeup with no remote IPI and no cross-core transfer of the queue.
///
/// The traffic that decides the cost either way is earlier and unavoidable: lane
/// bytes are written by the guest's own vCPU, so they cross a core boundary no
/// matter where the dispatcher sits.
///
/// A core chosen by a number is a guess about a layout this code cannot see —
/// `AXVISOR_DISPATCHER_CPU` exists so a deployment that knows where its guests
/// run can say so. A value outside this build's CPUs falls back to unpinned
/// rather than failing.
const DEFAULT_DISPATCHER_CPU_ID: usize = MANAGEMENT_CPU_ID;

/// The CPU a new output dispatcher pins itself to, if any.
///
/// Reads `[env] AXVISOR_DISPATCHER_CPU`: an absent value uses
/// [`DEFAULT_DISPATCHER_CPU_ID`], and `off` leaves the dispatcher unpinned so
/// the scheduler chooses.
fn dispatcher_cpu_id() -> Option<usize> {
    match option_env!("AXVISOR_DISPATCHER_CPU") {
        Some("off") => None,
        Some(value) => value.parse::<usize>().ok(),
        None => Some(DEFAULT_DISPATCHER_CPU_ID),
    }
}

static OUTPUT_HUB: NetworkOutputHub = NetworkOutputHub::new();
/// Which lane each guest owns. The table is written when a VM is created and
/// when one is removed, so the browser console set follows the VM registry
/// instead of a startup snapshot.
static LAYOUT: LazyLock<Mutex<Layout>> = LazyLock::new(|| Mutex::new(Layout::new()));

struct NetworkOutputHub {
    lanes: [NetworkOutputLane; CONSOLE_LANE_COUNT],
}

struct BrowserOutputDelivery {
    queue: Mutex<DeliveryQueue<OUTPUT_DELIVERY_QUEUE_CAPACITY>>,
    closed: AtomicBool,
    ready: IrqWaitCell,
}

impl NetworkOutputHub {
    const fn new() -> Self {
        Self {
            lanes: [const { NetworkOutputLane::new() }; CONSOLE_LANE_COUNT],
        }
    }

    fn submit(&self, lane: ConsoleLane, bytes: &[u8]) {
        self.lanes[lane.index()].submit(bytes);
    }

    fn is_connected(&self, lane: ConsoleLane) -> bool {
        self.lanes[lane.index()].is_connected()
    }

    fn begin_session(&self, lane: ConsoleLane) -> Option<usize> {
        self.lanes[lane.index()].begin_session()
    }

    fn end_session(&self, lane: ConsoleLane, session: usize) {
        self.lanes[lane.index()].end_session(session);
    }

    fn reset(&self, lane: ConsoleLane) {
        self.lanes[lane.index()].reset();
    }

    fn receive(
        &self,
        lane: ConsoleLane,
        session: usize,
        waiter: &IrqWorkerWaiter,
    ) -> Result<Option<NetworkOutputBatch>> {
        self.lanes[lane.index()].receive(session, waiter)
    }

    fn take_batch(&self, lane: ConsoleLane, session: usize) -> Option<NetworkOutputBatch> {
        self.lanes[lane.index()].take_batch(session)
    }
}

impl BrowserOutputDelivery {
    const fn new() -> Self {
        Self {
            queue: Mutex::new(DeliveryQueue::new()),
            closed: AtomicBool::new(false),
            ready: IrqWaitCell::new(),
        }
    }

    fn enqueue(&self, bytes: &[u8]) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let submitted = {
            let mut queue = self.queue.lock_unpoisoned();
            if self.closed.load(Ordering::Acquire) {
                false
            } else {
                queue.enqueue(bytes);
                true
            }
        };
        if submitted {
            let _result = self.ready.notify();
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let _result = self.ready.notify();
    }

    fn receive(&self, waiter: &IrqWorkerWaiter) -> Result<Option<Vec<u8>>> {
        loop {
            let mut bytes = [0; OUTPUT_DELIVERY_BATCH_CAPACITY];
            let (len, dropped_bytes) = self.queue.lock_unpoisoned().dequeue(&mut bytes);
            if len != 0 || dropped_bytes != 0 {
                let mut frame = DeliveryFrame::with_capacity(len + 96);
                frame.append(&bytes[..len], dropped_bytes);
                return Ok(Some(frame.into_bytes()));
            }
            if self.closed.load(Ordering::Acquire) {
                return Ok(None);
            }
            waiter
                .wait(&self.ready)
                .context("failed to wait for browser console delivery")?;
        }
    }
}

struct ActiveSession {
    lane: ConsoleLane,
    session: usize,
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        OUTPUT_HUB.end_session(self.lane, self.session);
    }
}

/// Makes `vm_id` visible as a browser console on the smallest free guest lane.
///
/// Called while the VM is being created, before it becomes visible in the
/// registry, so a full lane table fails that creation instead of yielding a VM
/// no browser can attach to. The failure is reported as
/// [`AxVmError::ResourceUnavailable`], which the HTTP control plane maps to 503
/// like any other exhausted host resource.
///
/// The result says whether this call took a lane or the VM already had one, so
/// a caller that has to undo the registration gives back only its own lane.
pub(crate) fn register_guest(vm_id: VMId, name: &str) -> Result<LaneAllocation> {
    let mut layout = LAYOUT.lock_unpoisoned();
    let allocation = layout.allocate(vm_id, name).map_err(|LayoutFull| {
        anyhow::anyhow!(AxVmError::ResourceUnavailable {
            resource: "browser console lane",
            detail: format!("all {MAX_GUEST_CONSOLES} guest console lanes are in use"),
        })
    })?;
    if allocation == LaneAllocation::Allocated {
        let lane = layout
            .guest(vm_id)
            .expect("allocated guest lane must be present")
            .lane;
        OUTPUT_HUB.reset(lane);
    }
    Ok(allocation)
}

/// Removes a guest's browser endpoint after its VM control task has stopped.
pub(crate) fn release_guest(vm_id: VMId) {
    let mut layout = LAYOUT.lock_unpoisoned();
    if let Some(endpoint) = layout.release(vm_id) {
        OUTPUT_HUB.reset(endpoint.lane);
    }
}

fn endpoints() -> Vec<Endpoint> {
    LAYOUT.lock_unpoisoned().endpoints()
}

fn endpoint_for_route(route: &str) -> Option<Endpoint> {
    LAYOUT.lock_unpoisoned().by_route(route)
}

fn lane_name(lane: ConsoleLane) -> String {
    LAYOUT
        .lock_unpoisoned()
        .by_lane(lane)
        .map(|endpoint| endpoint.display_name)
        .unwrap_or_else(|| format!("console lane {}", lane.index()))
}

/// Pins one web-console service task to the management CPU.
pub(crate) fn pin_current_task() {
    ax_set_current_affinity(AxCpuMask::one_shot(MANAGEMENT_CPU_ID))
        .expect("web console management CPU affinity must be valid");
}

/// Pins one output dispatcher to the CPU this build asks for.
///
/// The dispatcher waits on a lane and assembles WebSocket frames, so it runs
/// for every console session. Leaving it unpinned lets the scheduler place it
/// wherever it likes: measured on a four-core demo, that put successive
/// sessions on CPU 0, 2 and 3 and made the same keystroke cost anywhere from
/// 17 ms to 398 ms, because where the timer-driven task lands decides what its
/// wake-up waits behind.
///
/// The pin is a placement hint, not a requirement: a build whose CPU count does
/// not include the requested core keeps the dispatcher unpinned rather than
/// failing, so the same binary runs on a smaller machine.
fn pin_dispatcher_task() {
    let Some(cpu) = dispatcher_cpu_id() else {
        return;
    };
    let Ok(cpus) = thread::available_parallelism() else {
        return;
    };
    if cpu >= cpus.get() {
        warn!(
            "browser console dispatcher CPU {cpu} is outside this build's {cpus} CPUs; leaving it unpinned"
        );
        return;
    }
    if let Err(error) = ax_set_current_affinity(AxCpuMask::one_shot(cpu)) {
        warn!("browser console dispatcher CPU {cpu} affinity failed: {error:?}");
    }
}

/// Returns whether this browser route exists in the current layout.
pub(crate) fn has_console_route(route: &str) -> bool {
    endpoint_for_route(route).is_some()
}

/// Browser-visible console descriptors for the current VM set.
///
/// `attached` reports whether a browser session already holds the lane. The
/// lanes are exclusive, so a client that wants to explain its own failed
/// WebSocket upgrade needs this fact: the browser API hides the server's 409
/// behind an anonymous 1006 close, and a non-upgrade request never reaches the
/// upgrade handler at all.
pub(crate) fn console_descriptions() -> Vec<ConsoleDescription> {
    endpoints()
        .into_iter()
        .map(|endpoint| ConsoleDescription {
            route: endpoint.route,
            display_name: endpoint.display_name,
            attached: OUTPUT_HUB.is_connected(endpoint.lane),
        })
        .collect()
}

/// One console entry as the control plane reports it.
pub(crate) struct ConsoleDescription {
    pub(crate) route: String,
    pub(crate) display_name: String,
    pub(crate) attached: bool,
}

/// Copies Axvisor shell bytes into its fixed browser queue.
pub(crate) fn submit_management_output(bytes: &[u8]) {
    // The management shell's echo enters the lane here rather than through a
    // guest input queue, so this is the only mark of where a shell keystroke
    // started. Without it the shell's console path cannot be timed.
    OUTPUT_HUB.submit(ConsoleLane::MANAGEMENT, bytes);
}

/// Copies current guest output into its VM-specific fixed browser queue.
///
/// `SerialBackend::try_write` is called after the virtual UART has released
/// its register lock, from the vCPU task's ordinary polling/MMIO path. It is
/// not an interrupt callback, so the queue and dynamic lane table may use the
/// task-context mutex used by the rest of the current Axvisor runtime.
pub(crate) fn submit_guest_output(vm_id: VMId, bytes: &[u8]) {
    let layout = LAYOUT.lock_unpoisoned();
    let Some(lane) = layout.guest(vm_id).map(|endpoint| endpoint.lane) else {
        return;
    };
    OUTPUT_HUB.submit(lane, bytes);
}

/// Opens one in-process browser transport on an existing console lane.
pub(crate) fn open_browser_console(
    route: &str,
) -> Result<(BrowserConsoleInput, BrowserConsoleOutput)> {
    let (endpoint, active_session) = {
        let layout = LAYOUT.lock_unpoisoned();
        let endpoint = layout
            .by_route(route)
            .with_context(|| format!("unknown console endpoint `{route}`"))?;
        let session = OUTPUT_HUB.begin_session(endpoint.lane).with_context(|| {
            format!(
                "{} console already has an active session",
                endpoint.display_name
            )
        })?;
        let lane = endpoint.lane;
        (endpoint, ActiveSession { lane, session })
    };
    let lane = active_session.lane;
    let session = active_session.session;
    let delivery = start_output_dispatcher(lane, session)?;
    Ok((
        BrowserConsoleInput {
            endpoint,
            editor: ManagementLineEditor::new(),
            rejected_input_reported: false,
            _active_session: active_session,
        },
        BrowserConsoleOutput {
            delivery,
            waiter: None,
        },
    ))
}

fn start_output_dispatcher(
    lane: ConsoleLane,
    session: usize,
) -> Result<Arc<BrowserOutputDelivery>> {
    let delivery = Arc::new(BrowserOutputDelivery::new());
    let dispatcher_delivery = Arc::clone(&delivery);
    let task_name = format!("{}-browser-console-dispatcher", lane_name(lane));
    let worker_name = task_name.clone();
    std::thread::Builder::new()
        .name(task_name.clone())
        .spawn(move || {
            pin_dispatcher_task();
            if let Err(error) = run_output_dispatcher(lane, session, &dispatcher_delivery) {
                warn!("{worker_name} stopped: {error:#}");
            }
            dispatcher_delivery.close();
        })
        .with_context(|| format!("failed to start {task_name}"))?;
    Ok(delivery)
}

fn run_output_dispatcher(
    lane: ConsoleLane,
    session: usize,
    delivery: &BrowserOutputDelivery,
) -> Result<()> {
    let current = current_thread_handle()
        .context("failed to bind browser console dispatcher to its worker")?;
    let waiter = IrqWorkerWaiter::new(current.wake_handle());
    while let Some(frame) = receive_output_frame(lane, session, &waiter)? {
        delivery.enqueue(&frame);
    }
    Ok(())
}

fn receive_output_frame(
    lane: ConsoleLane,
    session: usize,
    waiter: &IrqWorkerWaiter,
) -> Result<Option<Vec<u8>>> {
    let Some(first) = OUTPUT_HUB.receive(lane, session, waiter)? else {
        return Ok(None);
    };
    let mut frame = DeliveryFrame::with_capacity(OUTPUT_FRAME_TARGET_CAPACITY);
    frame.append(&first.bytes[..first.len], first.dropped_bytes);

    // UART backends commonly submit one byte at a time. Coalesce for one
    // bounded interval in the dispatcher so the HTTP reactor handles frames,
    // not individual device writes.
    std::thread::sleep(OUTPUT_COALESCE_WINDOW);
    while frame.len() < OUTPUT_FRAME_TARGET_CAPACITY {
        let Some(batch) = OUTPUT_HUB.take_batch(lane, session) else {
            break;
        };
        frame.append(&batch.bytes[..batch.len], batch.dropped_bytes);
    }
    Ok(Some(frame.into_bytes()))
}

/// Input half of a board-hosted browser console session.
pub(crate) struct BrowserConsoleInput {
    endpoint: Endpoint,
    editor: ManagementLineEditor,
    /// Whether the current input streak was already rejected by a stopped guest.
    ///
    /// Keystrokes arrive one frame at a time, so reporting every rejected byte
    /// would turn one attempt into a wall of text; one notice per streak tells
    /// the operator why nothing is happening and stops on its own.
    rejected_input_reported: bool,
    _active_session: ActiveSession,
}

impl BrowserConsoleInput {
    /// Returns the session greeting sent before queued console output.
    pub(crate) fn greeting(&self) -> String {
        if let Some(vm_id) = self.endpoint.vm_id {
            format!("[Axvisor] browser console attached to VM {vm_id}\r\n")
        } else {
            format!(
                "Welcome to AxVisor Browser Shell!\r\nType 'help' for commands.\r\n{}",
                crate::shell::network_prompt()
            )
        }
    }

    /// Routes browser bytes to the selected shell and reports whether it stays open.
    pub(crate) fn route(&mut self, bytes: &[u8]) -> bool {
        if let Some(vm_id) = self.endpoint.vm_id {
            if crate::guest_console::route_network_input(vm_id, bytes) {
                self.rejected_input_reported = false;
            } else if !self.rejected_input_reported {
                // The guest console only accepts input while its VM runs. A
                // dropped keystroke used to leave no trace at all, which is
                // indistinguishable from a broken terminal: say why, once.
                self.rejected_input_reported = true;
                let notice = format!(
                    "[Axvisor] VM {vm_id} is not running; input was dropped. Start it first.\r\n"
                );
                submit_guest_output(vm_id, notice.as_bytes());
            }
            true
        } else {
            self.editor.process(bytes)
        }
    }
}

/// Blocking output half fed by the lane's fixed-capacity delivery queue.
pub(crate) struct BrowserConsoleOutput {
    delivery: Arc<BrowserOutputDelivery>,
    waiter: Option<IrqWorkerWaiter>,
}

impl BrowserConsoleOutput {
    /// Waits for one coalesced frame or session closure.
    pub(crate) fn receive(&mut self) -> Result<Option<Vec<u8>>> {
        if self.waiter.is_none() {
            let current = current_thread_handle()
                .context("failed to bind browser console output to its worker")?;
            self.waiter = Some(IrqWorkerWaiter::new(current.wake_handle()));
        }
        let waiter = self
            .waiter
            .as_ref()
            .expect("browser console waiter was initialized above");
        self.delivery.receive(waiter)
    }
}

struct ManagementLineEditor {
    line: [u8; MANAGEMENT_LINE_CAPACITY],
    len: usize,
    previous_was_cr: bool,
}

impl ManagementLineEditor {
    const fn new() -> Self {
        Self {
            line: [0; MANAGEMENT_LINE_CAPACITY],
            len: 0,
            previous_was_cr: false,
        }
    }

    fn process(&mut self, bytes: &[u8]) -> bool {
        for &byte in bytes {
            if byte == b'\n' && self.previous_was_cr {
                self.previous_was_cr = false;
                continue;
            }
            self.previous_was_cr = byte == b'\r';
            match byte {
                b'\r' | b'\n' => {
                    submit_management_output(b"\r\n");
                    let command = String::from_utf8_lossy(&self.line[..self.len]);
                    self.len = 0;
                    if !crate::shell::run_network_command(&command) {
                        submit_management_output(b"Goodbye!\r\n");
                        return false;
                    }
                    submit_management_output(crate::shell::network_prompt().as_bytes());
                }
                b'\x08' | b'\x7f' if self.len != 0 => {
                    self.len -= 1;
                    submit_management_output(b"\x08 \x08");
                }
                0x20..=0x7e if self.len < self.line.len() => {
                    self.line[self.len] = byte;
                    self.len += 1;
                    submit_management_output(&[byte]);
                }
                0x20..=0x7e => submit_management_output(b"\x07"),
                _ => {}
            }
        }
        true
    }
}
