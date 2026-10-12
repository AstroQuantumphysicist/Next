#![no_std]
#![no_main]

mod acpi;
mod apic;
mod apic_fault_injection;
mod apic_irq;
mod apic_irq_hardware;
mod apic_timer;
mod arch;
mod audio;
mod channel;
mod client_input_queue;
mod clock;
mod console_log;
mod event;
mod file;
mod filesystem;
mod hpet;
mod input;
mod kernelportal;
mod linux;
mod linux_fs;
mod memory_glue;
mod native;
mod native_surface;
mod network;
mod network_socket;
mod panic_report;
mod rtc;
mod storage;
mod storage_policy;
mod surface_buffer;
mod timer;
mod trace;
mod volumes;

use core::{
    arch::{asm, global_asm},
    fmt::{self, Write},
    panic::PanicInfo,
};
use next_abi::boot::{BootInfo, MemoryRegions};
use prism::{Canvas, EditorAction, PixelFormat, Shell};

struct Serial;
impl Write for Serial {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        input::write_serial(text);
        Ok(())
    }
}
macro_rules! log { ($($a:tt)*) => {{ let _ = writeln!(Serial, $($a)*); }}; }

// The ELF entry uses SysV64 and switches to a kernel-owned stack before Rust.
#[repr(align(4096))]
struct Stack([u8; 73728]);
#[no_mangle]
static mut KERNEL_STACK: Stack = Stack([0; 73728]);
const MAX_SCREEN_PIXELS: usize = 1920 * 1080;
#[repr(C, align(4096))]
struct ScreenBuffer([u32; MAX_SCREEN_PIXELS]);
static mut BACK_BUFFER: ScreenBuffer = ScreenBuffer([0; MAX_SCREEN_PIXELS]);
static mut SHELL_STORAGE: Shell = Shell::empty();
// The bounded semantic tree is caller-owned static memory, never a large
// temporary on a process/kernel exception stack. It grants no client rights.
static mut ACCESSIBILITY_SNAPSHOT: prism::accessibility::Snapshot =
    prism::accessibility::Snapshot::empty();
static mut ACCESSIBILITY_FRAME: u64 = 0;
struct ClientStaging {
    pixels: [u8; native_surface::BUFFER_BYTES],
    input_focus: Option<native_surface::NativeSurface>,
    input_suspend_pending: bool,
    present_pending: bool,
    visible_frame: [u64; native_surface::SURFACES],
    visible_configuration: [u64; native_surface::SURFACES],
    configure_wait: [Option<u64>; native_surface::SURFACES],
    configuration_presented:
        [Option<(native_surface::NativeSurface, u64, bool)>; native_surface::SURFACES],
    visible_ticket: [Option<native_surface::PublicationTicket>; native_surface::SURFACES],
    presented: [Option<(native_surface::NativeSurface, u64, native_surface::Origin)>;
        native_surface::SURFACES],
    window_presented: [Option<(
        native_surface::NativeSurface,
        prism::native_windows::Presentation,
    )>; native_surface::SURFACES],
    visible:
        [Option<(native_surface::NativeSurface, native_surface::Origin)>; native_surface::SURFACES],
    pending: [Option<(
        native_surface::NativeSurface,
        u64,
        native_surface::Origin,
        native_surface::PublicationTicket,
    )>; native_surface::SURFACES],
}
static mut CLIENT_STAGING: ClientStaging = ClientStaging {
    pixels: [0; native_surface::BUFFER_BYTES],
    input_focus: None,
    input_suspend_pending: false,
    present_pending: false,
    visible_frame: [0; native_surface::SURFACES],
    visible_configuration: [0; native_surface::SURFACES],
    configure_wait: [None; native_surface::SURFACES],
    configuration_presented: [None; native_surface::SURFACES],
    visible_ticket: [None; native_surface::SURFACES],
    presented: [None; native_surface::SURFACES],
    window_presented: [None; native_surface::SURFACES],
    visible: [None; native_surface::SURFACES],
    pending: [None; native_surface::SURFACES],
};

fn client_key(key: prism::Key) -> Option<next_abi::client_input::Key> {
    use next_abi::client_input::Key as Target;
    Some(match key {
        prism::Key::Character(c) => Target::Character(c),
        prism::Key::Enter => Target::Enter,
        prism::Key::Backspace => Target::Backspace,
        prism::Key::Tab => Target::Tab,
        prism::Key::Escape => Target::Escape,
        prism::Key::Up => Target::Up,
        prism::Key::Down => Target::Down,
        prism::Key::Left => Target::Left,
        prism::Key::Right => Target::Right,
        _ => return None,
    })
}

/// Runs only in the BSP desktop loop. Gestures refer to immutable broker IDs,
/// never an array index standing in for another client's authority.
fn handle_client_input(shell: &mut Shell) -> bool {
    use prism::client_interaction::Action;
    let staging = unsafe { &mut *core::ptr::addr_of_mut!(CLIENT_STAGING) };
    if !shell.client_input_allowed() && staging.input_focus.is_some() {
        staging.input_suspend_pending = true;
        shell.client_input_failed();
    }
    if staging.input_suspend_pending {
        if native_surface::suspend_input(timer::nanoseconds()).is_err() {
            return false;
        }
        staging.input_focus = None;
        staging.input_suspend_pending = false;
    }
    let mut changed = false;
    for _ in 0..16 {
        let Some(action) = shell.take_client_action() else {
            break;
        };
        changed = true;
        if action == Action::Suspend {
            staging.input_suspend_pending =
                native_surface::suspend_input(timer::nanoseconds()).is_err();
            staging.input_focus = None;
            if staging.input_suspend_pending {
                break;
            }
            continue;
        }
        let reference = match action {
            Action::Focus { surface }
            | Action::FocusChrome { surface }
            | Action::Key { surface, .. }
            | Action::Pointer { surface, .. }
            | Action::RequestClose { surface } => surface,
            Action::Suspend => unreachable!(),
        };
        let origin = native_surface::origins().ok().and_then(|origins| {
            origins.into_iter().flatten().find(|(id, _)| {
                id.slot() == reference.slot && id.generation() == reference.generation
            })
        });
        let Some((id, origin)) = origin else {
            let _ = shell.client_focus_result(reference, false);
            shell.client_input_failed();
            staging.input_suspend_pending = true;
            break;
        };
        match action {
            Action::Focus { .. } | Action::FocusChrome { .. } => {
                let accepted = native_surface::focus_gesture(id, timer::nanoseconds()).is_ok();
                let applied = shell.client_focus_result(reference, accepted);
                staging.input_focus = if accepted && applied { Some(id) } else { None };
                if accepted && !applied {
                    staging.input_suspend_pending = true;
                }
                log!(
                    "PRISM CLIENT FOCUS: slot={} generation={} pid={} accepted={}",
                    id.slot(),
                    id.generation(),
                    origin.pid,
                    accepted && applied
                );
                if accepted && applied {
                    if let Some(bounds) = shell.client_content_bounds(id.slot()) {
                        log!("PRISM CLIENT CONTENT: slot={} generation={} pid={} x={} y={} width={} height={}", id.slot(), id.generation(), origin.pid, bounds.x, bounds.y, bounds.width, bounds.height);
                    }
                }
            }
            Action::Key { key, .. } => {
                let accepted = client_key(key).is_some_and(|key| {
                    matches!(
                        native_surface::route_key(
                            id,
                            key,
                            next_abi::client_input::KeyAction::Press,
                            timer::nanoseconds()
                        ),
                        Ok(true)
                    )
                });
                if !accepted {
                    shell.client_input_failed();
                    staging.input_suspend_pending = true;
                }
                log!(
                    "PRISM CLIENT INPUT: slot={} generation={} pid={} kind=key accepted={}",
                    id.slot(),
                    id.generation(),
                    origin.pid,
                    accepted
                );
            }
            Action::Pointer { x, y, buttons, .. } => {
                if native_surface::pointer(id, x, y, buttons, timer::nanoseconds()).is_err() {
                    shell.client_input_failed();
                    staging.input_suspend_pending = true;
                }
            }
            Action::RequestClose { .. } => {
                let result = native_surface::request_close(id, timer::nanoseconds());
                let status = result
                    .as_ref()
                    .map_or(prism::client_surface::CloseStatus::Retry, |snapshot| {
                        client_close_status(snapshot.phase)
                    });
                shell.set_client_close_status(reference, status);
                if result.is_err() && matches!(native_surface::input_is_focused(id), Ok(false)) {
                    // An overflow may cancel focus. A transient lock refusal
                    // or duplicate request preserves the actual focused owner.
                    shell.client_input_failed();
                    staging.input_suspend_pending = true;
                }
                log!(
                    "PRISM CLIENT CLOSE: slot={} generation={} pid={} result={:?}",
                    id.slot(),
                    id.generation(),
                    origin.pid,
                    result
                );
            }
            Action::Suspend => unreachable!(),
        }
        if staging.input_suspend_pending {
            break;
        }
    }
    changed
}

fn client_close_status(phase: native_surface::ClosePhase) -> prism::client_surface::CloseStatus {
    use native_surface::ClosePhase as Phase;
    use prism::client_surface::CloseStatus as Status;
    match phase {
        Phase::Unsupported => Status::Unsupported,
        Phase::Idle => Status::Ready,
        Phase::Pending => Status::Requested,
        Phase::Closing => Status::Closing,
        Phase::Refused => Status::Refused,
        Phase::Unresponsive => Status::Unresponsive,
    }
}

/// Refresh advisory metadata only. The process owns its lifetime and may save
/// or refuse a request; expiry never grants the desktop authority to kill it.
fn refresh_client_close(shell: &mut Shell) -> bool {
    let staging = unsafe { &*core::ptr::addr_of!(CLIENT_STAGING) };
    let mut changed = false;
    for (id, origin) in staging.visible.iter().flatten() {
        let Ok(snapshot) = native_surface::close_snapshot(*id, timer::nanoseconds()) else {
            continue;
        };
        let reference = prism::client_interaction::SurfaceRef {
            slot: id.slot(),
            generation: id.generation(),
        };
        let status = client_close_status(snapshot.phase);
        let previous = shell.client_close_status(reference);
        // A queue/admission failure remains visible until a new explicit retry
        // or an authoritative lifecycle change, rather than vanishing pre-frame.
        if previous == Some(prism::client_surface::CloseStatus::Retry)
            && snapshot.phase == native_surface::ClosePhase::Idle
        {
            continue;
        }
        if previous.is_some_and(|old| old != status) {
            changed |= shell.set_client_close_status(reference, status);
            log!(
                "PRISM CLIENT CLOSE STATUS: slot={} generation={} pid={} status={:?}",
                id.slot(),
                id.generation(),
                origin.pid,
                status
            );
            if let Some(bounds) = shell.client_close_button_bounds(id.slot()) {
                log!("PRISM CLIENT CLOSE BUTTON: slot={} generation={} pid={} x={} y={} width={} height={}",
                    id.slot(), id.generation(), origin.pid, bounds.x, bounds.y, bounds.width, bounds.height);
            }
        }
    }
    changed
}

/// Only a released trusted chrome ticket can change broker configuration.
fn handle_native_resize(shell: &mut Shell) -> bool {
    use prism::native_resize::ResizeError;
    let Some(action) = shell.take_native_resize_action() else {
        return false;
    };
    let fail = |shell: &mut Shell, error| {
        shell.native_resize_result(action.request, Err(error));
        log!(
            "PRISM CLIENT RESIZE REFUSED: request={} slot={} generation={} error={:?}",
            action.request,
            action.surface.slot,
            action.surface.generation,
            error
        );
    };
    if shell.client_configuration(action.surface) != Some(action.expected_configuration)
        || !shell.client_content_available(action.surface)
    {
        fail(shell, ResizeError::Stale);
        return true;
    }
    if !(16..=128).contains(&action.width)
        || !(16..=128).contains(&action.height)
        || action.width.checked_mul(4) != Some(action.stride)
        || u64::from(action.stride) * u64::from(action.height) > native_surface::BUFFER_BYTES as u64
    {
        fail(shell, ResizeError::Invalid);
        return true;
    }
    let origins = match native_surface::origins() {
        Ok(origins) => origins,
        Err(_) => {
            fail(shell, ResizeError::Retry);
            return true;
        }
    };
    let Some((id, origin)) = origins.into_iter().flatten().find(|(id, _)| {
        id.slot() == action.surface.slot && id.generation() == action.surface.generation
    }) else {
        fail(shell, ResizeError::Stale);
        return true;
    };
    let staged = unsafe { &*core::ptr::addr_of!(CLIENT_STAGING) };
    if staged.visible[id.slot()].is_none_or(|(live, _)| live != id)
        || staged.visible_configuration[id.slot()] != action.expected_configuration
        || staged.configure_wait[id.slot()].is_some()
    {
        fail(shell, ResizeError::Stale);
        return true;
    }
    let geometry = prism::surface_state::Geometry {
        logical_width: action.width,
        logical_height: action.height,
        pixel_width: action.width,
        pixel_height: action.height,
        scale_16_16: 65536,
    };
    let result = native_surface::reconfigure_trusted(
        id,
        action.expected_configuration,
        geometry,
        timer::nanoseconds(),
    );
    match result {
        Err(status) => {
            let error = match status {
                next_abi::Status::WouldBlock => ResizeError::Retry,
                next_abi::Status::InvalidHandle => ResizeError::Stale,
                next_abi::Status::AccessDenied => ResizeError::Denied,
                _ => ResizeError::Invalid,
            };
            fail(shell, error);
        }
        Ok(configuration) => {
            let new = configuration.configure.generation;
            let staging = unsafe { &mut *core::ptr::addr_of_mut!(CLIENT_STAGING) };
            let slot = id.slot();
            // Keep the live owner and last actually shown frame. The previous
            // ticket and content cease to authorize any new presentation.
            staging.pending[slot] = None;
            staging.visible_ticket[slot] = None;
            staging.configure_wait[slot] = Some(new);
            if staging.input_focus == Some(id) {
                staging.input_focus = None;
            }
            let retired = shell
                .retire_client_content(action.surface, action.expected_configuration, new)
                .is_ok();
            let applied = retired && shell.native_resize_result(action.request, Ok(new));
            staging.present_pending = true;
            log!("PRISM CLIENT RESIZE: request={} slot={} generation={} pid={} old_configuration={} configuration={} width={} height={} stride={} applied={}",
                action.request, id.slot(), id.generation(), origin.pid,
                action.expected_configuration, new, action.width, action.height, action.stride, applied);
        }
    }
    true
}

struct ClientUpdates {
    repaint: bool,
    shown: [Option<(native_surface::NativeSurface, u64, native_surface::Origin)>;
        native_surface::SURFACES],
    cleared: [Option<(native_surface::NativeSurface, u64)>; native_surface::SURFACES],
}
impl ClientUpdates {
    fn changed(&self) -> bool {
        self.repaint
            || self.shown.iter().any(Option::is_some)
            || self.cleared.iter().any(Option::is_some)
    }
}

fn consume_client_pending(
    slot: usize,
    staging: &mut ClientStaging,
    shell: &mut Shell,
    updates: &mut ClientUpdates,
) {
    let Some((id, frame, origin, ticket)) = staging.pending[slot] else {
        return;
    };
    match native_surface::consume(id, ticket) {
        Ok(()) => {
            staging.pending[slot] = None;
            updates.shown[slot] = Some((id, frame, origin));
            updates.repaint = true;
        }
        Err(next_abi::Status::InvalidHandle) => {
            match native_surface::configure(id, origin.owner) {
                Ok(_) => {
                    // Retired publications do not imply a dead surface. Leave
                    // its identity intact; the frame fence retires old content
                    // for a newer configure or waits for the newer publication.
                    staging.pending[slot] = None;
                    staging.present_pending = true;
                    updates.repaint = true;
                }
                Err(next_abi::Status::InvalidHandle) => {
                    let _ = shell.clear_client_surface(slot, id.generation());
                    staging.pending[slot] = None;
                    if staging.visible[slot].is_some_and(|(live, _)| live == id) {
                        staging.visible[slot] = None;
                        staging.visible_ticket[slot] = None;
                        staging.configure_wait[slot] = None;
                    }
                    updates.cleared[slot] = Some((id, origin.pid));
                }
                Err(_) => {
                    // An unavailable ownership lookup cannot authorize teardown.
                }
            }
        }
        Err(_) => {
            // Temporary broker refusal preserves the accepted copy/ticket.
            // Retry consumption before submitting this generation again.
        }
    }
}

// BSP desktop only. Pixel staging stays in retained storage, outside the
// 64KiB IRQ stack, and never borrows a producer's userspace mapping.
fn handle_client_surfaces(shell: &mut Shell) -> ClientUpdates {
    let mut updates = ClientUpdates {
        repaint: false,
        shown: [None; native_surface::SURFACES],
        cleared: [None; native_surface::SURFACES],
    };
    let Ok(origins) = native_surface::origins() else {
        return updates;
    };
    let staging = unsafe { &mut *core::ptr::addr_of_mut!(CLIENT_STAGING) };
    for slot in 0..native_surface::SURFACES {
        if let Some((id, origin)) = staging.visible[slot] {
            if !origins.iter().flatten().any(|(live, _)| *live == id) {
                let _ = shell.clear_client_surface(slot, id.generation());
                staging.visible[slot] = None;
                staging.visible_ticket[slot] = None;
                staging.configure_wait[slot] = None;
                staging.pending[slot] = None;
                if staging.input_focus == Some(id) {
                    staging.input_focus = None;
                }
                updates.cleared[slot] = Some((id, origin.pid));
            }
        }
        if staging.pending[slot].is_some() {
            consume_client_pending(slot, staging, shell, &mut updates);
            continue;
        }
        let Ok(Some((id, frame, origin, ticket))) = native_surface::peek(slot, &mut staging.pixels)
        else {
            continue;
        };
        if !matches!(native_surface::is_live(id), Ok(true)) {
            continue;
        }
        let Ok(len) = usize::try_from(frame.buffer_bytes) else {
            continue;
        };
        let Some(pixels) = staging.pixels.get(..len) else {
            continue;
        };
        let format = match frame.format {
            next_abi::surface::PixelFormat::Rgbx => PixelFormat::Rgb,
            next_abi::surface::PixelFormat::Bgrx => PixelFormat::Bgr,
        };
        if shell
            .submit_client_surface_configured(
                slot,
                id.generation(),
                frame.frame_generation,
                frame.configure_generation,
                origin.pid,
                origin.package_id(),
                frame.pixel_width as usize,
                frame.pixel_height as usize,
                (frame.stride_bytes / 4) as usize,
                format,
                pixels,
            )
            .is_err()
        {
            // A rejected UI copy leaves the publication pending, so an app
            // cannot bypass backpressure through malformed display metadata.
            continue;
        }
        staging.visible[slot] = Some((id, origin));
        staging.visible_frame[slot] = frame.frame_generation;
        staging.visible_configuration[slot] = frame.configure_generation;
        staging.configure_wait[slot] = None;
        staging.visible_ticket[slot] = Some(ticket);
        staging.pending[slot] = Some((id, frame.frame_generation, origin, ticket));
        updates.repaint = true;
        consume_client_pending(slot, staging, shell, &mut updates);
    }
    updates
}
struct EditorBaseline {
    path: Option<prism::EditorPath>,
    bytes: [u8; 4096],
    len: usize,
    exists: bool,
}
static mut EDITOR_BASELINE: EditorBaseline = EditorBaseline {
    path: None,
    bytes: [0; 4096],
    len: 0,
    exists: false,
};
fn log_snap_events(shell: &mut Shell) {
    while let Some(event) = shell.take_snap_event() {
        log!(
            "PRISM SNAP: kind={:?} app={:?} zone={:?} x={} y={} width={} height={}",
            event.kind,
            event.app,
            event.zone,
            event.bounds.x,
            event.bounds.y,
            event.bounds.width,
            event.bounds.height
        );
    }
}
// Trusted desktop actions never accept a caller-provided path or identity.
#[inline(never)]
fn handle_file_portals(shell: &mut Shell, displayed: &mut Option<u64>) -> bool {
    let mut changed = false;
    while let Some(action) = shell.take_file_portal_action() {
        let result = match action {
            prism::FilePortalAction::Approve { request, path } => {
                let access = kernelportal::first_pending()
                    .filter(|pending| pending.id == request)
                    .map(|pending| pending.access);
                let result = access
                    .ok_or(next_abi::Status::InvalidHandle)
                    .and_then(|rights| kernelportal::approve(request, path.as_str(), rights));
                log!(
                    "PRISM PORTAL APPROVE: request={} path={} result={:?}",
                    request,
                    path.as_str(),
                    result
                );
                result
            }
            prism::FilePortalAction::Deny { request } => {
                let result = kernelportal::deny(request);
                log!("PRISM PORTAL DENY: request={} result={:?}", request, result);
                result
            }
        };
        if result.is_err() {
            shell.append_output("File consent could not be completed; request was revalidated.");
        }
        // The shell has dismissed the action's dialog. Redisplay only a request
        // still pending after authoritative validation.
        *displayed = None;
        changed = true;
    }
    let pending = kernelportal::first_pending();
    let next = pending.map(|request| request.id);
    if *displayed != next {
        if let Some(old) = displayed.take() {
            shell.dismiss_file_portal(old);
        }
        if let Some(request) = pending {
            refresh_files(shell);
            let summaries = native::summaries();
            let package = summaries
                .iter()
                .find(|p| p.pid == request.pid)
                .map(|p| p.package_id())
                .unwrap_or("unavailable requester");
            shell.show_file_portal(request.id, request.pid, package, request.access == 3);
            log!(
                "PRISM PORTAL: request={} pid={} access={}",
                request.id,
                request.pid,
                request.access
            );
            if let Some(path) = shell.file_portal_selected_path() {
                log!("PRISM PORTAL SELECTED: path={}", path);
            }
        }
        *displayed = next;
        changed = true;
    }
    changed
}
// Only the kernel console loop accesses this state; user tasks cannot address it.
#[inline(never)]
fn handle_editor_actions(shell: &mut Shell) {
    let baseline = unsafe { &mut *core::ptr::addr_of_mut!(EDITOR_BASELINE) };
    while let Some(action) = shell.take_editor_action() {
        match action {
            EditorAction::Open { path } => {
                let mut bytes = [0u8; 4096];
                let (len, exists) = match file::read_path(path.as_str(), &mut bytes) {
                    Ok(len) => (len, true),
                    Err(next_abi::Status::NotFound) if path.as_str().starts_with("/native/") => {
                        (0, false)
                    }
                    Err(_) => {
                        shell.editor_load_failed("Cannot read document.");
                        continue;
                    }
                };
                let Ok(text) = core::str::from_utf8(&bytes[..len]) else {
                    shell.editor_load_failed("Document is not valid UTF-8.");
                    continue;
                };
                if shell.load_document(path.as_str(), text).is_ok() {
                    baseline.path = Some(path);
                    baseline.bytes = bytes;
                    baseline.len = len;
                    baseline.exists = exists;
                    log!(
                        "PRISM EDITOR OPEN: path={} bytes={} exists={}",
                        path.as_str(),
                        len,
                        exists
                    );
                } else {
                    shell.editor_load_failed("Unsaved changes; document retained.");
                }
            }
            EditorAction::Save {
                path,
                data,
                len,
                revision,
            } => {
                if !path.as_str().starts_with("/native/") || baseline.path != Some(path) {
                    shell.editor_save_result(
                        revision,
                        Err("Open a writable /native/ document first."),
                    );
                    continue;
                }
                let mut current = [0u8; 4096];
                let unchanged = match file::read_path(path.as_str(), &mut current) {
                    Ok(n) => {
                        baseline.exists && n == baseline.len && current[..n] == baseline.bytes[..n]
                    }
                    Err(next_abi::Status::NotFound) => !baseline.exists,
                    Err(_) => {
                        shell.editor_save_result(
                            revision,
                            Err("Cannot verify current file; edits retained."),
                        );
                        continue;
                    }
                };
                if !unchanged {
                    shell.editor_save_result(
                        revision,
                        Err("File changed since loading; edits retained."),
                    );
                    log!("PRISM EDITOR CONFLICT: path={}", path.as_str());
                    continue;
                }
                match file::replace(path.as_str(), &data[..len]) {
                    Ok(()) => {
                        baseline.bytes = data;
                        baseline.len = len;
                        baseline.exists = true;
                        shell.editor_save_result(revision, Ok(()));
                        refresh_files(shell);
                        log!(
                            "PRISM EDITOR SAVE: path={} bytes={} revision={}",
                            path.as_str(),
                            len,
                            revision
                        );
                    }
                    Err(_) => {
                        shell.editor_save_result(revision, Err("Save failed; edits retained."))
                    }
                }
            }
        }
    }
}
// Construct separately so the main boot frame contains a pointer, rather than
// every application's document/history buffers across nested boot calls.
#[inline(never)]
unsafe fn initialize_shell() -> &'static mut Shell {
    let shell = &mut *core::ptr::addr_of_mut!(SHELL_STORAGE);
    shell.initialize_defaults();
    shell
}
global_asm!(
    ".section .text.entry,\"ax\"",
    ".global helix_start",
    "helix_start:",
    "cli",
    "cld",
    "lea rsp, [rip + KERNEL_STACK + 69632]",
    "and rsp, -16",
    "xor ebp, ebp",
    "call helix_entry",
    "2: cli",
    "hlt",
    "jmp 2b",
    ".global next_exception",
    "next_exception:",
    "cli",
    "cld",
    "and rsp, -16",
    "call helix_fault",
    "3: hlt",
    "jmp 3b",
    ".text",
);
extern "sysv64" {
    fn next_exception();
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Gate {
    low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    middle: u16,
    high: u32,
    zero: u32,
}
const EMPTY_GATE: Gate = Gate {
    low: 0,
    selector: 0,
    ist: 0,
    flags: 0,
    middle: 0,
    high: 0,
    zero: 0,
};
#[repr(C, align(16))]
struct Idt([Gate; 256]);
static mut IDT: Idt = Idt([EMPTY_GATE; 256]);
#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}
unsafe fn install_exception_diagnostics() {
    let selector: u16;
    asm!("mov {0:x}, cs", out(reg) selector, options(nomem, nostack, preserves_flags));
    let address = next_exception as *const () as u64;
    let gate = Gate {
        low: address as u16,
        selector,
        ist: 0,
        flags: 0x8e,
        middle: (address >> 16) as u16,
        high: (address >> 32) as u32,
        zero: 0,
    };
    let ptr = core::ptr::addr_of_mut!(IDT.0) as *mut Gate;
    for i in 0..256 {
        ptr.add(i).write(gate);
    }
    let idtr = Idtr {
        limit: 4095,
        base: ptr as u64,
    };
    asm!("lidt [{}]", in(reg) &idtr, options(readonly, nostack, preserves_flags));
}

/// Entry of the independently loaded, fixed-address development ELF kernel.
/// # Safety
/// Loader must pass an aligned, readable BootInfo and keep occupied memory mapped.
#[no_mangle]
pub unsafe extern "sysv64" fn helix_entry(h: *const BootInfo) -> ! {
    let _stack_base = core::ptr::addr_of!(KERNEL_STACK.0);
    input::init_serial();
    install_exception_diagnostics();
    log!("HELIX: ExitBootServices succeeded; independent ELF kernel control");
    if h.is_null() || !(h as usize).is_multiple_of(core::mem::align_of::<BootInfo>()) {
        log!("HELIX: invalid boot pointer");
        halt();
    }
    let boot = *h;
    if let Err(e) = boot.validate() {
        log!("HELIX: invalid boot handoff: {:?}", e);
        halt();
    }
    for (base, size) in [
        (boot.framebuffer.base, boot.framebuffer.byte_len),
        (boot.memory_map.base, boot.memory_map.byte_len),
        (boot.kernel.base, boot.kernel.byte_len),
        (boot.loader.base, boot.loader.byte_len),
        (boot.initrd.base, boot.initrd.byte_len),
    ] {
        if base.checked_add(size).is_none_or(|end| end > 1u64 << 32) {
            log!("HELIX: unsupported resource above bootstrap 4 GiB mapping");
            halt();
        }
    }
    let bytes = core::slice::from_raw_parts(
        boot.memory_map.base as *const u8,
        boot.memory_map.byte_len as usize,
    );
    let regions = match MemoryRegions::parse(bytes, boot.memory_map.descriptor_size as usize) {
        Ok(value) => value,
        Err(e) => {
            log!("HELIX: invalid memory regions: {:?}", e);
            halt();
        }
    };
    for range in [
        boot.kernel,
        boot.loader,
        boot.initrd,
        next_abi::boot::PhysicalRange {
            base: boot.memory_map.base,
            byte_len: boot.memory_map.byte_len,
        },
    ] {
        if !regions.is_loader_owned(range) {
            log!("HELIX: invalid occupied-memory ownership");
            halt();
        }
    }
    if (h as u64) < boot.loader.base
        || (h as u64)
            .checked_add(core::mem::size_of::<BootInfo>() as u64)
            .is_none_or(|end| end > boot.loader.base + boot.loader.byte_len)
    {
        log!("HELIX: invalid BootInfo ownership");
        halt();
    }
    match boot.ap_trampoline(&regions) {
        Ok(Some(range)) => log!("HELIX AP RESERVATION PASS base={:#x} bytes={}; loader-owned page retained, APs remain stopped", range.base, range.byte_len),
        Ok(None) => log!("HELIX AP RESERVATION UNAVAILABLE; BSP operation retained"),
        Err(error) => {
            log!("HELIX AP RESERVATION REJECT {:?}", error);
            halt();
        }
    }
    match boot.ap_bootstrap_scratch(&regions) {
        Ok(Some(range)) => log!("HELIX AP SCRATCH PASS base={:#x} bytes={}; private low bootstrap storage retained, APs remain stopped", range.base, range.byte_len),
        Ok(None) => log!("HELIX AP SCRATCH UNAVAILABLE; BSP operation retained"),
        Err(error) => {
            log!("HELIX AP SCRATCH REJECT {:?}", error);
            halt();
        }
    }
    let ram_bytes = (0..regions.len())
        .filter_map(|i| regions.get(i))
        .filter(|r| matches!(r.kind, 1 | 2 | 3 | 4 | 7))
        .fold(0u64, |sum, r| {
            sum.saturating_add(r.pages.saturating_mul(4096))
        });
    log!(
        "HELIX: validated {} memory regions; RAM {} MiB; ACPI {:#x}",
        regions.len(),
        ram_bytes / 1048576,
        boot.acpi_rsdp
    );
    log!("HELIX: BOOTINFO SELFTEST PASS checksum, version, kernel/loader/map reservations; init image retained");
    let _ = trace::set_filter(trace::Filter::ALL);
    kernel_checks(&regions);
    if let Err(error) = arch::run_user_demo() {
        log!("HELIX USER FAILED: {:?}", error);
        halt();
    }
    match timer::start() {
        Ok(()) => log!(
            "HELIX: TIMER SELFTEST PASS PIT IRQ0, monotonic ticks={}, integer/XMM0 sentinels preserved; no IRQ allocation or locks",
            timer::ticks()
        ),
        Err(error) => {
            log!("HELIX TIMER FAILED: {}", error);
            halt();
        }
    }
    match clock::initialize() {
        Ok(()) => log!(
            "HELIX UTC PASS seconds={} source=q35-CMOS/PIT; second-precision initial anchor",
            clock::realtime().map_or(0, |value| value.seconds)
        ),
        Err(error) => log!("HELIX UTC UNAVAILABLE {:?}; no fabricated realtime", error),
    }
    match acpi::discover(boot.acpi_rsdp, &regions) {
        Ok(topology) => {
            log!(
                "HELIX ACPI PASS cpus={} io_apics={} overrides={} tables={} bsp={}; discovery only",
                topology.cpu_count,
                topology.io_apic_count,
                topology.override_count,
                topology.validated_tables,
                topology.bootstrap_apic_id
            );
            let local_apic = apic::probe_local(&topology, &regions);
            match local_apic {
                Ok(info) => log!(
                    "HELIX LAPIC PASS id={} version={} address={:#x}; read-only discovery",
                    info.apic_id,
                    info.version,
                    info.address
                ),
                Err(error) => log!("HELIX LAPIC UNAVAILABLE {:?}; PIC retained", error),
            }
            let io_apic = apic::probe_io(&topology, &regions);
            match io_apic {
                Ok(info) => {
                    let pins: u32 = info.controllers[..info.len]
                        .iter()
                        .map(|controller| controller.pins as u32)
                        .sum();
                    let encoding_valid = info.plan.routes[..info.plan.len].iter().all(|route| {
                        info.controllers[route.controller as usize]
                            .redirection_registers(route.pin)
                            .is_ok()
                            && apic::masked_entry(
                                *route,
                                32 + route.source,
                                info.plan.destination as u32,
                            )
                            .is_ok()
                    });
                    if !encoding_valid {
                        log!("HELIX IOAPIC UNAVAILABLE invalid masked routing plan; PIC retained");
                    } else {
                        log!("HELIX IOAPIC PASS controllers={} pins={} isa_routes={}; redirection untouched, PIC retained", info.len, pins, info.plan.len);
                    }
                }
                Err(error) => log!("HELIX IOAPIC UNAVAILABLE {:?}; PIC retained", error),
            }
            if let Some(desc) = topology.hpet {
                match hpet::probe(desc, &regions) {
                    Ok(info) => {
                        log!("HELIX HPET PASS frequency={} timers={} advanced={} elapsed_ns={}; PIT remains scheduler clock", info.frequency_hz, info.timers, info.counter_ticks, info.elapsed_ns(info.counter_ticks));
                        if let Ok(local) = local_apic {
                            let state = apic_timer::inspect(local);
                            log!("HELIX LAPIC TIMER STATE lvt={:#x} divide={:#x} initial={} current={} svr={:#x} pending={}", state.lvt, state.divide, state.initial, state.current, state.svr, state.pending_vector);
                            match apic_timer::calibrate(local, info, &regions) {
                                Ok(calibration) => match calibration.counts_for_ns(1_000_000) {
                                    Ok(count) => log!("HELIX LAPIC TIMER PASS divider={} divided_hz={} samples={:?} count_1ms={} adopted={}; masked firmware timer left stopped, PIT retained", calibration.divider, calibration.divided_hz, calibration.samples, count, calibration.adopted_firmware_timer),
                                    Err(error) => log!("HELIX LAPIC TIMER UNAVAILABLE {:?}; PIT retained", error),
                                },
                                Err(error) => log!("HELIX LAPIC TIMER UNAVAILABLE {:?}; PIT retained", error),
                            }
                            if let Ok(io) = &io_apic {
                                let gate = arch::install_apic_spurious_gate();
                                match apic_irq_hardware::route_pit(io, local, info, &regions, gate)
                                {
                                    Ok(report) => log!(
                                        "HELIX IOAPIC IRQ RESULT {:?}; PIT remains time source",
                                        report
                                    ),
                                    Err(error) => {
                                        log!("HELIX IOAPIC IRQ ERROR {:?}", error);
                                        if error.fatal() {
                                            halt();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => log!("HELIX HPET UNAVAILABLE {:?}; PIT retained", error),
                }
            }
        }
        Err(error) => log!(
            "HELIX ACPI UNAVAILABLE {:?}; single CPU development profile retained",
            error
        ),
    }
    let mut file_content = [0u8; 512];
    let mut file_len = 0usize;
    match storage::probe_and_read_boot_sector() {
        Ok(disk) => {
            log!("HELIX DISK SELFTEST PASS read-only virtio-blk {} sectors; boot-signature={} FAT32-BPB={}", disk.capacity_sectors, disk.boot_signature, disk.fat32_bpb);
            if !disk.fat32_bpb || disk.protective_mbr {
                match volumes::probe(disk.capacity_sectors, &mut file_content) {
                    Ok(discovery) => {
                        log!("HELIX GPT DISCOVERY PASS esp_start={} esp_blocks={} data_start={} data_blocks={} note_bytes={}; read-only discovery", discovery.esp.first_lba, discovery.esp.block_count(), discovery.data.first_lba, discovery.data.block_count(), discovery.note_bytes);
                        if let Ok(note) =
                            core::str::from_utf8(&file_content[..discovery.note_bytes])
                        {
                            log!("HELIX GPT ESP NOTE: {}", note.trim());
                        }
                        let initrd = core::slice::from_raw_parts(
                            boot.initrd.base as *const u8,
                            boot.initrd.byte_len as usize,
                        );
                        let extent = match storage_policy::AuthenticatedPolicy::load(initrd)
                            .and_then(|policy| {
                                policy.bind(
                                    discovery.disk_guid,
                                    discovery.data,
                                    disk.capacity_sectors,
                                )
                            }) {
                            Ok(extent) => extent,
                            Err(error) => {
                                log!(
                                    "HELIX GPT STORE REJECT {:?}; no formatting or repair",
                                    error
                                );
                                halt();
                            }
                        };
                        match filesystem::initialize_authenticated_extent(
                            disk.capacity_sectors,
                            extent,
                        ) {
                            Ok(info) => {
                                log!("HELIX GPT STORE PASS generation={} read_only={} formatted={} selftest={}; authenticated prefix mount", info.generation, info.read_only, info.formatted, info.self_test);
                                file_len = discovery.note_bytes;
                            }
                            Err(error) => {
                                log!("HELIX GPT STORE REJECT {}; no formatting or repair", error);
                                halt();
                            }
                        }
                    }
                    Err(error) => {
                        log!(
                            "HELIX GPT DISCOVERY REJECT {:?}; no formatting or repair",
                            error
                        );
                        halt();
                    }
                }
            } else {
                // The builder reserves the trailing MiB outside the FAT volume.
                // Restrict this probe to that explicit development geometry.
                match filesystem::initialize(disk.capacity_sectors) {
                    Ok(info) => {
                        log!(
                        "HELIX COW STORE PASS generation={} read_only={} formatted={} selftest={}",
                        info.generation,
                        info.read_only,
                        info.formatted,
                        info.self_test
                    );
                        // initialize validated the explicit development disk
                        // geometry/tag. Never probe-write an unknown/read-only disk.
                        if !info.read_only && disk.capacity_sectors == 65 * 2048 {
                            match storage_write_probe(disk.capacity_sectors - 1) {
                    Ok(()) => log!("HELIX STORAGE WRITE PASS dedicated trailing sector write/read/flush/restore"),
                    Err(error) => {
                        log!("HELIX STORAGE WRITE FAILED {:?}",error);
                        halt();
                    }
                }
                        }
                    }
                    Err(error) => {
                        log!("HELIX COW STORE FAILED {}", error);
                        halt();
                    }
                }
                match next_fs::Volume::mount(BootDisk {
                    sectors: disk.capacity_sectors,
                }) {
                    Ok(mut volume) => match volume.read_file("/NEXT.TXT", &mut file_content) {
                        Ok(len) => {
                            file_len = len;
                            log!(
                                "HELIX FS SELFTEST PASS /NEXT.TXT {} bytes read from FAT32",
                                len
                            );
                        }
                        Err(error) => log!("HELIX FS UNAVAILABLE: {:?}", error),
                    },
                    Err(error) => log!("HELIX FS UNAVAILABLE: {:?}", error),
                }
            }
        }
        Err(error) => log!("HELIX DISK UNAVAILABLE: {:?}", error),
    }
    match network::init() {
        Ok(info) => {
            log!("HELIX DHCP PASS address={:?} mask={:?} gateway={:?} dns={:?} server={:?} lease_seconds={} renew={} rebind={}",info.lease.address,info.lease.subnet_mask,info.lease.gateway,info.lease.dns,info.lease.server,info.lease.lease_seconds,info.lease.renew_after,info.lease.rebind_after);
            log!("HELIX NETWORK PASS MAC={:02x?} gateway={:02x?} ARP={} ICMP={} UDP={}; QEMU development profile",info.mac,info.gateway_mac,info.arp_reply,info.icmp_reply,info.udp_reply);
            if let Some(candidate) = info.dhcpv6_candidate {
                log!("HELIX DHCPV6 CANDIDATE PASS addresses={} T1={} T2={}; validated peer exchange only, not installed, no DAD or route",candidate.address_count(),candidate.t1_seconds,candidate.t2_seconds);
            } else {
                log!("HELIX DHCPV6 UNAVAILABLE; optional bounded candidate exchange");
            }
            if info.dns_reply && info.dns_cache {
                log!("HELIX DNS PASS A/AAAA=true cache=true; local development peer, bounded retry and TTL expiry");
            } else {
                log!("HELIX DNS UNAVAILABLE; optional local development peer");
            }
            log!(
                "HELIX TCP PASS HTTP={}; optional local QEMU peer",
                info.tcp_reply
            );
        }
        Err(error) => {
            log!("HELIX NETWORK FAILED {}", error);
            halt();
        }
    }
    match audio::init() {
        Ok(info)=>log!("HELIX AUDIO PASS sample_rate={} frames={} frequency={} completed={}; AC97 development profile",info.sample_rate,info.frames,info.frequency,info.completed),
        Err(error)=>{log!("HELIX AUDIO FAILED {}",error);halt();}
    }
    let init_bytes =
        core::slice::from_raw_parts(boot.initrd.base as *const u8, boot.initrd.byte_len as usize);
    if let Err(error) = native::initialize(init_bytes) {
        log!("HELIX: NATIVE INIT FAILED {:?}", error);
        halt();
    }
    let startup_deadline = timer::nanoseconds().saturating_add(5_000_000_000);
    while !native::ready() {
        input::pump_serial();
        if timer::nanoseconds() >= startup_deadline {
            log!("HELIX: NATIVE STARTUP FAILED supervisor readiness deadline");
            halt();
        }
        if !native::step() {
            // `step` returns with interrupts enabled. Keep the enable and
            // halt adjacent so an IRQ cannot be fully handled between the
            // idle decision and HLT, where it would otherwise be missed until
            // the next periodic timer tick.
            asm!("sti; hlt", options(nomem, nostack));
        }
    }
    trace::record(
        trace::Category::Boot,
        trace::Severity::Info,
        trace::id::BOOT_READY,
        0,
        [timer::ticks(), 1, 0, 0],
    );
    log!("HELIX: NATIVE SELFTEST PASS separate processes, IPC, capabilities, contained faults and timer preemption");
    let fb = &boot.framebuffer;
    let front = core::slice::from_raw_parts_mut(fb.base as *mut u32, fb.byte_len as usize / 4);
    let Some(screen_pixels) = (fb.stride as usize)
        .checked_mul(fb.height as usize)
        .filter(|count| *count <= MAX_SCREEN_PIXELS)
    else {
        log!("PRISM: framebuffer exceeds bounded 1080p development back buffer");
        halt();
    };
    let pixels = core::slice::from_raw_parts_mut(
        core::ptr::addr_of_mut!(BACK_BUFFER.0).cast::<u32>(),
        screen_pixels,
    );
    let mut frame_generation = 0u64;
    let mut canvas = match Canvas::new(
        pixels,
        fb.width as usize,
        fb.height as usize,
        fb.stride as usize,
        if fb.pixel_format == next_abi::boot::PIXEL_BGR {
            PixelFormat::Bgr
        } else {
            PixelFormat::Rgb
        },
    ) {
        Ok(value) => value,
        Err(error) => {
            log!("PRISM: invalid framebuffer: {:?}", error);
            halt();
        }
    };
    let shell = initialize_shell();
    shell.set_viewport(fb.width as usize, fb.height as usize);
    load_appearance(shell);
    shell.set_system_line(
        0,
        "Single CPU / own signed deployment / unsigned EFI loader",
    );
    shell.set_system_line(1, "PIT timer / legacy virtio / PS/2 + serial");
    shell.set_system_line(2, "DHCP / Ethernet ARP + ICMP / optional UDP and TCP peers");
    shell.set_system_line(
        3,
        "Bounded COW data store / no installer or certified hardware",
    );
    if file_len > 0 {
        if let Ok(note) = core::str::from_utf8(&file_content[..file_len]) {
            shell.set_boot_file(note);
        }
    }
    refresh_files(shell);
    let mouse_ready = input::init_mouse(fb.width, fb.height);
    log!("PRISM: PS2 MOUSE ready={}", mouse_ready);
    let mut previous_buttons = 0u8;
    shell.set_status("ELF kernel | native processes + IPC | PIT clock | development build");
    present(shell, &mut canvas, front, &mut frame_generation);
    log!(
        "PRISM: READY {}x{}; commands help about status uptime clear; serial + PS/2 input",
        fb.width,
        fb.height
    );
    let mut displayed_portal = None;
    let mut notification_dropped = 0;
    loop {
        input::pump_serial();
        network_socket::pump();
        // Preserve runnable native work across the desktop poll loop. Sleeping
        // for a PIT tick after every short dispatch can starve CPU-bound users
        // of their configured share and distort HPET budget enforcement.
        let ran_native = native::step();
        let mut output_changed = handle_file_portals(shell, &mut displayed_portal)
            || unsafe { (*core::ptr::addr_of!(CLIENT_STAGING)).present_pending };
        output_changed |= handle_appearance(shell);
        output_changed |= handle_files_gui(shell);
        let client_updates = handle_client_surfaces(shell);
        output_changed |= client_updates.changed();
        output_changed |= handle_native_resize(shell);
        output_changed |= handle_client_input(shell);
        output_changed |= refresh_client_close(shell);
        while let Some(record) = native::take_notification() {
            let mut title = TextBuffer::<64>::new();
            let mut body = TextBuffer::<192>::new();
            let _ = write!(title, "Process {} exited", record.pid);
            let _ = write!(
                body,
                "Exit status {}. Executable SHA-256: ",
                record.exit_code
            );
            for byte in record.executable_digest {
                let _ = write!(body, "{byte:02x}");
            }
            let priority = if record.exit_code == 0 {
                prism::notifications::Priority::Info
            } else {
                prism::notifications::Priority::Warning
            };
            let published = shell.publish_notification(
                prism::notifications::Source::Process { pid: record.pid },
                record.package_id(),
                title.as_str(),
                body.as_str(),
                priority,
                record.monotonic_ns,
            );
            log!(
                "PRISM: process notification pid={} exit={} accepted={}",
                record.pid,
                record.exit_code,
                published.is_ok()
            );
            output_changed = true;
        }
        let dropped = native::notification_dropped();
        if dropped != notification_dropped {
            log!("PRISM: process notification queue overwritten={}", dropped);
            notification_dropped = dropped;
        }
        while let Some(record) = console_log::pop() {
            let mut label = TextBuffer::<64>::new();
            let _ = write!(label, "[Process {}]", record.pid);
            shell.append_output(label.as_str());
            shell.append_output(record.text());
            output_changed = true;
        }
        if console_log::take_dropped() != 0 {
            shell.append_output("Some process output was dropped: terminal queue is full.");
            output_changed = true;
        }
        if output_changed && present(shell, &mut canvas, front, &mut frame_generation) {
            log!("PRISM OUTPUT FRAME: generation={}", frame_generation);
        }
        if let Some(event) = input::poll_event() {
            let (key, serial) = match event {
                input::InputEvent::InspectAccessibility => {
                    dump_accessibility();
                    continue;
                }
                input::InputEvent::Key(key) => (key, false),
                input::InputEvent::SerialKey(key) => (key, true),
                input::InputEvent::Mouse(mouse) => {
                    shell.on_pointer_packet(mouse.x, mouse.y, mouse.buttons);
                    let buttons_changed = mouse.buttons != previous_buttons;
                    if buttons_changed {
                        log!(
                            "PRISM POINTER: x={} y={} buttons={} focused={:?}",
                            mouse.x,
                            mouse.y,
                            mouse.buttons,
                            shell.desktop().focused()
                        );
                    }
                    previous_buttons = mouse.buttons;
                    log_snap_events(shell);
                    handle_file_portals(shell, &mut displayed_portal);
                    handle_editor_actions(shell);
                    handle_appearance(shell);
                    handle_files_gui(shell);
                    handle_native_resize(shell);
                    handle_client_input(shell);
                    let presented = present(shell, &mut canvas, front, &mut frame_generation);
                    if presented {
                        log!(
                            "PRISM POINTER PRESENTED: generation={} x={} y={} buttons={}",
                            frame_generation,
                            mouse.x,
                            mouse.y,
                            mouse.buttons
                        );
                        if buttons_changed {
                            log!("PRISM FRAME: generation={}", frame_generation);
                        }
                    }
                    continue;
                }
            };
            let portal_active = shell.file_portal_active();
            let modal_active = portal_active
                || shell.snap_preview().is_some()
                || shell.native_resize_preview().is_some()
                || shell.notifications_open()
                || shell.file_dialog_active()
                || shell.file_query_dialog_active()
                || shell.trash_dialog_active();
            if !serial && shell.client_context_active() {
                let modifiers = input::modifiers();
                // This version exposes semantic presses, not modifier states.
                // Unsupported modified text must never become plain client text.
                if (modifiers.ctrl || modifiers.alt) && client_key(key).is_some() {
                    handle_client_input(shell);
                    continue;
                }
            }
            if !serial && !modal_active && !shell.client_context_active() {
                let modifiers = input::modifiers();
                if modifiers.ctrl
                    && matches!(key, prism::Key::Character('s' | 'S'))
                    && shell.desktop().focused() == Some(prism::App::Editor)
                {
                    shell.request_editor_save();
                    handle_editor_actions(shell);
                    if present(shell, &mut canvas, front, &mut frame_generation) {
                        log!("PRISM FRAME: generation={}", frame_generation);
                    }
                    continue;
                }
                if modifiers.ctrl && matches!(key, prism::Key::Character('l' | 'L')) {
                    shell.clear_terminal();
                    present(shell, &mut canvas, front, &mut frame_generation);
                    continue;
                }
                if modifiers.alt && key == prism::Key::Tab {
                    shell.on_key(prism::Key::Tab);
                    present(shell, &mut canvas, front, &mut frame_generation);
                    continue;
                }
            }
            if serial && !modal_active {
                shell.client_leave_context();
                handle_client_input(shell);
                shell.open_app(prism::App::Terminal);
            }
            if !modal_active
                && (serial || !shell.launcher_open())
                && (serial || !shell.client_context_active())
                && shell.desktop().focused() == Some(prism::App::Terminal)
                && shell.desktop().editing().is_none()
            {
                match key {
                    prism::Key::Character(ch) if ch.is_ascii() => {
                        let mut buf = [0; 4];
                        input::write_serial(ch.encode_utf8(&mut buf));
                    }
                    prism::Key::Backspace if !shell.input().is_empty() => {
                        input::write_serial("\x08 \x08")
                    }
                    prism::Key::Enter => {
                        input::write_serial("\n");
                        let mut response = TextBuffer::<4096>::new();
                        macro_rules! reply { ($($a:tt)*) => {{ log!($($a)*); let _=writeln!(response,$($a)*); }}; }
                        match shell.input().trim() {
                            "help" => reply!("NEXT: help about status uptime clear ps run PATH kill PID; native processes and IPC active; bounded COW store and VM Ethernet; development build"),
                            "about" => reply!("NEXT: Helix + Prism original Rust development kernel; not Next 1.0"),
                            "status" => reply!("NEXT: kernel-control=independent boot-info=validated framebuffer=active input=polling"),
                            "trace" => {
                                let _ = trace::dump(&mut response);
                                let _ = Serial.write_str(response.as_str());
                            },
                            "ps" => {
                                for process in native::summaries().into_iter().filter(|p| p.pid != 0) {
                                    reply!("NEXT PROCESS: pid={} state={} preemptions={} console={}", process.pid, process.state, process.preemptions, process.console_owned);
                                    reply!("NEXT IDENTITY: pid={} package={} signer={:02x?} digest={:02x?} dev-grant={:#x} capability-ceiling={:#x}", process.pid, process.package_id(), process.signer_prefix, process.digest_prefix, process.developer_grant, process.capability_ceiling);
                                }
                            },
                            command if command.starts_with("run ") => {
                                match native::console_spawn(command[4..].trim()) {
                                    Ok(pid) => reply!("NEXT RUN: pid={}", pid),
                                    Err(error) => reply!("NEXT RUN FAILED: {:?}", error),
                                }
                            },
                            command if command.starts_with("runfiles ") => {
                                match native::console_spawn_with_files(command[9..].trim()) {
                                    Ok(pid) => reply!("NEXT RUNFILES: pid={}", pid),
                                    Err(error) => reply!("NEXT RUNFILES FAILED: {:?}", error),
                                }
                            },
                            command if command.starts_with("kill ") => {
                                match command[5..].trim().parse::<u64>() {
                                    Ok(pid) => match native::console_kill(pid) {
                                        Ok(()) => reply!("NEXT KILL: pid={}",pid),
                                        Err(error) => reply!("NEXT KILL FAILED: {:?}",error),
                                    },
                                    Err(_) => reply!("NEXT KILL FAILED: InvalidArgument"),
                                }
                            },
                            command if command == "edit" || command.starts_with("edit ") => {},
                            "clear" => reply!("NEXT: terminal cleared"),
                            "uart" => {
                                input::pump_serial();
                                let metrics = input::serial_metrics();
                                reply!("NEXT UART: accepted={} transmitted={} dropped={} stalled={} absent={} pending={}", metrics.accepted, metrics.transmitted, metrics.dropped, metrics.stalled, metrics.absent, metrics.pending);
                            },
                            "date" => match clock::realtime() {
                                Ok(value) => reply!("NEXT UTC: seconds={} nanoseconds={} source=q35-CMOS/PIT", value.seconds, value.nanoseconds),
                                Err(error) => reply!("NEXT UTC UNAVAILABLE: {:?}", error),
                            },
                            "uptime" => reply!("NEXT CLOCK: ticks={} nanoseconds={} source=PIT divisor={}", timer::ticks(), timer::nanoseconds(), timer::PIT_DIVISOR),
                            command if command == "calc" || command.starts_with("calc ") => {
                                match prism::calculator::evaluate(command.strip_prefix("calc").unwrap_or("").trim()) {
                                    Ok(value) => reply!("= {}",value),
                                    Err(error) => reply!("{}",error.message()),
                                }
                            },
                            "cat /NEXT.TXT" if file_len > 0 => {
                                let text = core::str::from_utf8(&file_content[..file_len]).unwrap_or("invalid file encoding");
                                reply!("NEXT FILE: {}",text.trim());
                            },
                            "cat /NEXT.TXT" => reply!("NEXT FILE: unavailable"),
                            "" => {},
                            command => match filesystem::command(command) {
                                Some(result) => reply!("NEXT DATA: {}",result.as_str()),
                                None => reply!("NEXT: unknown command"),
                            },
                        }
                        if !response.as_str().is_empty() {
                            shell.set_command_response(response.as_str());
                        }
                    }
                    _ => {}
                }
            }
            if key == prism::Key::Enter && shell.desktop().focused() == Some(prism::App::Terminal) {
                refresh_files(shell);
            }
            shell.set_uptime_ns(timer::nanoseconds());
            if serial && !modal_active {
                shell.on_terminal_key(key);
            } else {
                shell.on_key(key);
            }
            if !serial
                && matches!(
                    key,
                    prism::Key::SnapLeft
                        | prism::Key::SnapRight
                        | prism::Key::SnapMaximize
                        | prism::Key::SnapRestore
                        | prism::Key::Menu
                        | prism::Key::Tab
                        | prism::Key::BackTab
                        | prism::Key::WindowNext
                        | prism::Key::WindowPrevious
                        | prism::Key::WindowSwitchEnd
                        | prism::Key::OpenSearch
                        | prism::Key::OpenSettings
                        | prism::Key::OpenFiles
                        | prism::Key::OpenTerminal
                        | prism::Key::CloseWindow
                        | prism::Key::Notifications
                        | prism::Key::Delete
                        | prism::Key::Enter
                        | prism::Key::Escape
                        | prism::Key::Up
                        | prism::Key::Down
                        | prism::Key::Left
                        | prism::Key::Right
                )
            {
                log!(
                    "PRISM DESKTOP: focused={:?} home={} editing={:?}",
                    shell.desktop().focused(),
                    shell.launcher_open(),
                    shell.desktop().editing()
                );
            }
            if portal_active && matches!(key, prism::Key::Up | prism::Key::Down) {
                if let Some(path) = shell.file_portal_selected_path() {
                    log!("PRISM PORTAL SELECTED: path={}", path);
                }
            }
            log_snap_events(shell);
            handle_file_portals(shell, &mut displayed_portal);
            handle_editor_actions(shell);
            handle_appearance(shell);
            handle_files_gui(shell);
            handle_native_resize(shell);
            handle_client_input(shell);
            for (index, process) in native::summaries().into_iter().enumerate() {
                if process.pid != 0 {
                    shell.set_monitor_process(
                        index,
                        process.pid,
                        process.state,
                        process.preemptions,
                    );
                } else {
                    shell.set_monitor_line(index, "");
                }
            }
            let presented = present(shell, &mut canvas, front, &mut frame_generation);
            if presented
                && matches!(
                    key,
                    prism::Key::Enter
                        | prism::Key::SnapLeft
                        | prism::Key::SnapRight
                        | prism::Key::SnapMaximize
                        | prism::Key::SnapRestore
                        | prism::Key::Menu
                        | prism::Key::Tab
                        | prism::Key::BackTab
                        | prism::Key::WindowNext
                        | prism::Key::WindowPrevious
                        | prism::Key::WindowSwitchEnd
                        | prism::Key::OpenSearch
                        | prism::Key::OpenSettings
                        | prism::Key::OpenFiles
                        | prism::Key::OpenTerminal
                        | prism::Key::CloseWindow
                        | prism::Key::Notifications
                        | prism::Key::Delete
                        | prism::Key::Escape
                        | prism::Key::Up
                        | prism::Key::Down
                        | prism::Key::Left
                        | prism::Key::Right
                )
            {
                log!("PRISM FRAME: generation={}", frame_generation);
            }
        } else if !ran_native {
            // Timer IRQ wakes this polling development console within one PIT
            // period only when no native quantum ran. Drain available input
            // before sleeping to avoid UART loss. The adjacent STI/HLT closes
            // the lost-wakeup window between the empty event poll and sleep.
            asm!("sti; hlt", options(nomem, nostack));
        }
    }
}

/// Development diagnostic reads only the last completed static tree. F9 is
/// consumed by the BSP before UI dispatch, including under trusted dialogs.
fn dump_accessibility() {
    let snapshot = unsafe { &*core::ptr::addr_of!(ACCESSIBILITY_SNAPSHOT) };
    let frame = unsafe { ACCESSIBILITY_FRAME };
    if !snapshot.complete || frame == 0 {
        log!("PRISM A11Y UNAVAILABLE: frame={}", frame);
        return;
    }
    log!(
        "PRISM A11Y BEGIN: frame={} generation={} context={} token={} count={} focused={}",
        frame,
        snapshot.generation,
        snapshot.context,
        snapshot.token,
        snapshot.nodes().len(),
        snapshot.focused().map_or(0, |id| id.local)
    );
    for node in snapshot.nodes() {
        let mut line = TextBuffer::<768>::new();
        let _ = write!(line, "PRISM A11Y NODE: id={} parent={} role={:?} enabled={} visible={} focused={} selected={} checked={} busy={} bounds=",
            node.id.local, node.parent.map_or(0, |id| id.local), node.role,
            u8::from(node.state.enabled), u8::from(node.state.visible), u8::from(node.state.focused),
            u8::from(node.state.selected), node.state.checked.map_or(-1, i8::from), u8::from(node.state.busy));
        if let Some(bounds) = node.bounds {
            let _ = write!(
                line,
                "{},{},{},{}",
                bounds.x, bounds.y, bounds.width, bounds.height
            );
        } else {
            let _ = write!(line, "none");
        }
        let _ = write!(line, " name_hex=");
        for byte in node.name().bytes() {
            let _ = write!(line, "{:02x}", byte);
        }
        log!("{}", line.as_str());
    }
    log!(
        "PRISM A11Y END: frame={} generation={} count={}",
        frame,
        snapshot.generation,
        snapshot.nodes().len()
    );
}

// Fence both unconsumed staging and already-visible geometry against the exact
// publication/configuration. A lock refusal defers the whole framebuffer copy.
fn quiesce_client_publications(shell: &mut Shell) -> bool {
    let staging = unsafe { &mut *core::ptr::addr_of_mut!(CLIENT_STAGING) };
    for slot in 0..native_surface::SURFACES {
        let Some((id, origin)) = staging.visible[slot] else {
            continue;
        };
        let reference = prism::client_interaction::SurfaceRef {
            slot: id.slot(),
            generation: id.generation(),
        };
        let configuration = match native_surface::configure(id, origin.owner) {
            Ok(configuration) => configuration.configure.generation,
            Err(next_abi::Status::InvalidHandle) => {
                let _ = shell.clear_client_surface(slot, id.generation());
                staging.visible[slot] = None;
                staging.visible_ticket[slot] = None;
                staging.pending[slot] = None;
                staging.configure_wait[slot] = None;
                if staging.input_focus == Some(id) {
                    staging.input_focus = None;
                    staging.input_suspend_pending = true;
                }
                continue;
            }
            Err(_) => {
                staging.present_pending = true;
                return false;
            }
        };
        let expected = staging.visible_configuration[slot];
        if configuration > expected {
            // Reconfiguration retires content, never the live object's identity.
            // Repeat safely after a transient failure or a superseding configure.
            let local = shell.client_configuration(reference).unwrap_or(expected);
            let retire_expected = if local < configuration {
                local
            } else {
                expected
            };
            let already_retired =
                local == configuration && !shell.client_content_available(reference);
            if !already_retired
                && shell
                    .retire_client_content(reference, retire_expected, configuration)
                    .is_err()
            {
                staging.present_pending = true;
                return false;
            }
            staging.pending[slot] = None;
            staging.visible_ticket[slot] = None;
            staging.configure_wait[slot] = Some(configuration);
            if staging.input_focus == Some(id) {
                staging.input_focus = None;
            }
            continue;
        }
        if configuration != expected {
            staging.present_pending = true;
            return false;
        }
        let result = staging.visible_ticket[slot]
            .ok_or(next_abi::Status::InvalidHandle)
            .and_then(|ticket| native_surface::publication_is_current(id, ticket));
        match result {
            Ok(true) => {}
            // A newer publication from a live same-config owner must be copied
            // next iteration. It cannot turn a temporary fence into teardown.
            Ok(false) | Err(_) => {
                staging.present_pending = true;
                return false;
            }
        }
    }
    true
}

fn log_client_presented(shell: &Shell, generation: u64) {
    let staging = unsafe { &mut *core::ptr::addr_of_mut!(CLIENT_STAGING) };
    for slot in 0..native_surface::SURFACES {
        let current =
            staging.visible[slot].map(|(id, origin)| (id, staging.visible_frame[slot], origin));
        if let Some((old, _, origin)) = staging.presented[slot] {
            if current.is_none_or(|(id, _, _)| id != old) {
                log!(
                    "PRISM SURFACE CLEARED: slot={} generation={} pid={}",
                    old.slot(),
                    old.generation(),
                    origin.pid
                );
                staging.presented[slot] = None;
            }
        }
        let placement = current.and_then(|(id, _, _)| {
            shell
                .native_window_presentation(prism::client_interaction::SurfaceRef {
                    slot: id.slot(),
                    generation: id.generation(),
                })
                .map(|placement| (id, placement))
        });
        if let Some((id, p)) = placement {
            if staging.window_presented[slot] != placement {
                log!(
                    "PRISM NATIVE WINDOW PRESENTED: frame={} slot={} generation={} mapped={} minimized={} x={} y={} width={} height={} z={} focused={}",
                    generation, id.slot(), id.generation(), u8::from(p.mapped),
                    u8::from(p.minimized), p.bounds.x, p.bounds.y, p.bounds.width,
                    p.bounds.height, p.z, u8::from(p.focused)
                );
            }
            // Hidden windows retain their owner, latest publication and last
            // shown frame. Only a mapped window can claim new pixels presented.
            if p.mapped && staging.configure_wait[slot].is_none() {
                if let Some((id, frame, origin)) = current {
                    if staging.presented[slot]
                        .is_none_or(|(old, old_frame, _)| old != id || old_frame != frame)
                    {
                        log!(
                            "PRISM SURFACE: slot={} generation={} frame={} pid={} package={}",
                            id.slot(),
                            id.generation(),
                            frame,
                            origin.pid,
                            origin.package_id()
                        );
                    }
                    staging.presented[slot] = current;
                }
            }
        }
        let configuration = current.map(|(id, _, _)| {
            (
                id,
                staging.configure_wait[slot].unwrap_or(staging.visible_configuration[slot]),
                staging.configure_wait[slot].is_some(),
            )
        });
        if configuration != staging.configuration_presented[slot] {
            if let Some((id, configuration, pending)) = configuration {
                log!("PRISM CLIENT CONFIGURATION PRESENTED: frame={} slot={} generation={} configuration={} pending={}",
                    generation, id.slot(), id.generation(), configuration, u8::from(pending));
            }
        }
        staging.configuration_presented[slot] = configuration;
        staging.window_presented[slot] = placement;
    }
    staging.present_pending = false;
}

fn present(
    shell: &mut Shell,
    canvas: &mut Canvas<'_>,
    front: &mut [u32],
    generation: &mut u64,
) -> bool {
    if !quiesce_client_publications(shell) {
        return false;
    }
    let semantic = unsafe { &mut *core::ptr::addr_of_mut!(ACCESSIBILITY_SNAPSHOT) };
    let semantic_result = shell.write_accessibility_snapshot(semantic);
    shell.render(canvas);
    if canvas.present_to(front).is_err() {
        log!("PRISM: framebuffer presentation bounds failed");
        halt();
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    *generation = generation.saturating_add(1);
    log_client_presented(shell, *generation);
    unsafe {
        ACCESSIBILITY_FRAME = *generation;
    }
    if let Err(error) = semantic_result {
        log!(
            "PRISM ACCESSIBILITY UNAVAILABLE: frame={} error={:?}",
            generation,
            error
        );
    }
    for _ in 0..8 {
        let Some(event) = shell.take_accessibility_event() else {
            break;
        };
        log!(
            "PRISM ACCESSIBILITY: frame={} generation={} kind={:?} focused={:?}",
            generation,
            event.generation,
            event.kind,
            event.focused
        );
    }
    true
}

struct AppearanceDisk;
impl prism::appearance::Persistence for AppearanceDisk {
    type Error = next_abi::Status;
    #[inline(never)]
    fn read(&mut self, path: &str, output: &mut [u8; 32]) -> Result<Option<usize>, Self::Error> {
        let mut bytes = [0; next_fs::journal::MAX_FILE_BYTES];
        match file::read_path(path, &mut bytes) {
            Ok(len) => {
                let count = len.min(output.len());
                output[..count].copy_from_slice(&bytes[..count]);
                Ok(Some(len))
            }
            Err(next_abi::Status::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn replace_atomic(&mut self, path: &str, bytes: &[u8; 32]) -> Result<(), Self::Error> {
        if path != prism::appearance::PATH {
            return Err(next_abi::Status::AccessDenied);
        }
        file::replace_preferences(bytes)
    }
}
#[inline(never)]
fn load_appearance(shell: &mut Shell) {
    match prism::appearance::load(&mut AppearanceDisk) {
        Ok(loaded) => {
            let origin = loaded.origin;
            let accepted = shell.load_appearance(loaded).is_ok();
            log!(
                "PRISM APPEARANCE LOAD: origin={:?} accepted={}",
                origin,
                accepted
            );
        }
        Err(error) => {
            shell.appearance_load_error();
            log!(
                "PRISM APPEARANCE LOAD FAILED {:?}; original record preserved",
                error
            );
        }
    }
}
#[inline(never)]
fn handle_appearance(shell: &mut Shell) -> bool {
    let Some(request) = shell.take_appearance_request() else {
        return false;
    };
    let mut active = shell.active_appearance();
    let result = prism::appearance::save(&mut AppearanceDisk, &mut active, request.preferences);
    let accepted = shell.appearance_save_result(request.id, result.is_ok());
    log!(
        "PRISM APPEARANCE SAVE: request={} theme={:?} accent={:?} result={:?} accepted={}",
        request.id,
        request.preferences.theme,
        request.preferences.accent,
        result,
        accepted
    );
    true
}

fn refresh_file_bindings(shell: &mut Shell) {
    if let Ok(revision) = filesystem::generation() {
        shell.set_files_revision(revision);
    }
    match filesystem::snapshot() {
        Ok(entries) => {
            let count = entries.iter().filter(|file| file.valid).count();
            let bytes: u64 = entries
                .iter()
                .filter(|file| file.valid)
                .map(|file| u64::from(file.size))
                .sum();
            let mut description = TextBuffer::<128>::new();
            let _ = write!(
                description,
                "Data store: {} files / {} verified bytes",
                count, bytes
            );
            shell.set_system_line(4, description.as_str());
            for (index, file) in entries.into_iter().enumerate() {
                if file.valid {
                    shell.set_file(
                        index + 1,
                        file.path(),
                        if file.binary {
                            "Binary file"
                        } else {
                            file.text()
                        },
                    );
                } else {
                    shell.clear_file(index + 1);
                }
            }
        }
        Err(_) => {
            for index in 1..=8 {
                shell.clear_file(index);
            }
        }
    }
}

fn query_files(
    query: prism::files_query::Query,
) -> Result<
    (prism::files_query::Stats, [prism::files_actions::Path; 8]),
    prism::files_actions::BackendError,
> {
    use prism::files_actions::{BackendError, Path};
    let snapshot = filesystem::query_snapshot(query).map_err(|error| {
        log!("PRISM FILE QUERY BACKEND: {:?}", error);
        match error {
            filesystem::FsError::Busy => BackendError::Busy,
            filesystem::FsError::Unavailable => BackendError::Unavailable,
            _ => BackendError::Io,
        }
    })?;
    let placeholder = Path::new("/native").map_err(|_| BackendError::InvalidPath)?;
    let mut paths = [placeholder; 8];
    let mut count = 0;
    for file in snapshot.files.iter().filter(|file| file.valid) {
        let Some(slot) = paths.get_mut(count) else {
            return Err(BackendError::Io);
        };
        *slot = Path::new(file.path()).map_err(|_| BackendError::InvalidPath)?;
        count += 1;
    }
    if count != snapshot.stats.matched {
        return Err(BackendError::Io);
    }
    Ok((snapshot.stats, paths))
}

fn refresh_files(shell: &mut Shell) {
    refresh_file_bindings(shell);
    match query_files(shell.file_query().applied()) {
        Ok((stats, paths)) => {
            if shell
                .refresh_file_query(stats, &paths[..stats.matched])
                .is_err()
            {
                shell.file_query_load_error(prism::files_actions::BackendError::Conflict);
            }
        }
        Err(error) => shell.file_query_load_error(error),
    }
    match file::list_trash() {
        Ok(listing) => {
            let _ = shell.set_trash_listing(listing);
        }
        Err(error) => shell.trash_load_error(error),
    }
    match file::trash_undo_available() {
        Ok(receipt) => {
            let _ = shell.set_trash_undo_receipt(receipt);
        }
        Err(error) => shell.trash_undo_load_error(error),
    }
}

#[inline(never)]
fn handle_files_gui(shell: &mut Shell) -> bool {
    let mut changed = false;
    for _ in 0..4 {
        let Some(action) = shell.take_files_directory_action() else {
            break;
        };
        let result = file::directory_action(action);
        log!(
            "PRISM DIRECTORY ACTION: request={} kind={:?} result={:?}",
            action.request,
            action.intent,
            result.as_ref().map(|listing| listing.folder.revision)
        );
        let committed = result.is_ok();
        let _ = shell.files_directory_action_result(action.request, result);
        if committed {
            refresh_files(shell);
        }
        changed = true;
    }
    for _ in 0..4 {
        let Some(action) = shell.take_file_query_action() else {
            break;
        };
        refresh_file_bindings(shell);
        let result = query_files(action.query);
        log!(
            "PRISM FILE QUERY: request={} sort={:?} descending={} search={:?} result={:?}",
            action.request,
            action.query.sort,
            action.query.descending,
            action.query.search,
            result.as_ref().map(|(stats, _)| stats)
        );
        let completed = match result {
            Ok((stats, paths)) => {
                shell.file_query_result(action.request, Ok(stats), &paths[..stats.matched])
            }
            Err(error) => shell.file_query_result(action.request, Err(error), &[]),
        };
        if completed.is_err() {
            shell.file_query_load_error(prism::files_actions::BackendError::Conflict);
        }
        refresh_files(shell);
        changed = true;
    }
    for _ in 0..4 {
        let Some(action) = shell.take_file_action() else {
            break;
        };
        match action {
            prism::files_actions::Action::InspectDestination { request, path } => {
                let result = file::inspect_gui(path);
                log!(
                    "PRISM FILE INSPECT: request={} path={} result={:?}",
                    request,
                    path.as_str(),
                    result
                );
                let _ = shell.file_destination_result(request, result);
            }
            prism::files_actions::Action::Commit { request, intent } => {
                let result = file::commit_gui(intent);
                log!(
                    "PRISM FILE COMMIT: request={} intent={:?} result={:?}",
                    request,
                    intent,
                    result
                );
                let _ = shell.file_action_result(request, result);
                if result.is_ok() {
                    refresh_files(shell);
                }
            }
        }
        changed = true;
    }
    while let Some(outcome) = shell.take_file_outcome() {
        log!("PRISM FILE OUTCOME: {:?}", outcome);
        changed = true;
    }
    for _ in 0..4 {
        let Some(action) = shell.take_trash_action() else {
            break;
        };
        let result = file::commit_trash(action.intent);
        log!(
            "PRISM TRASH COMMIT: request={} intent={:?} result={:?}",
            action.request,
            action.intent,
            result
        );
        let _ = shell.trash_action_result(action.request, result);
        if result.is_ok() {
            refresh_files(shell);
        }
        changed = true;
    }
    for _ in 0..4 {
        let Some(action) = shell.take_trash_undo_action() else {
            break;
        };
        let result = file::undo_trash(action.generation);
        log!(
            "PRISM TRASH UNDO: request={} generation={} result={:?}",
            action.request,
            action.generation,
            result
        );
        let _ = shell.trash_undo_result(action.request, result);
        refresh_files(shell);
        changed = true;
    }
    changed
}

struct TextBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> TextBuffer<N> {
    fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl<const N: usize> Write for TextBuffer<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let mut count = value.len().min(N - self.len);
        while !value.is_char_boundary(count) {
            count -= 1;
        }
        self.bytes[self.len..self.len + count].copy_from_slice(&value.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

/// Exercise only the generated data region, preserving its original sector.
unsafe fn storage_write_probe(lba: u64) -> Result<(), storage::StorageError> {
    let mut original = [0; 512];
    storage::read_sector(lba, &mut original)?;
    let pattern = core::array::from_fn(|index| (index as u8).wrapping_mul(37).wrapping_add(11));
    storage::write_sector(lba, &pattern)?;
    storage::flush()?;
    let mut actual = [0; 512];
    let result = storage::read_sector(lba, &mut actual);
    storage::write_sector(lba, &original)?;
    storage::flush()?;
    result?;
    if actual != pattern {
        return Err(storage::StorageError::InvalidCompletion);
    }
    Ok(())
}

struct BootDisk {
    sectors: u64,
}
impl next_fs::BlockDevice for BootDisk {
    type Error = storage::StorageError;
    fn read_sector(&mut self, lba: u64, output: &mut [u8; 512]) -> Result<(), Self::Error> {
        // Created only after the boot CPU claims the identity-mapped device.
        unsafe { storage::read_sector(lba, output) }
    }
    fn sector_count(&self) -> u64 {
        self.sectors
    }
}

fn kernel_checks(regions: &MemoryRegions<'_>) {
    use helix_core::{
        handle::{HandleError, HandleTable, Rights},
        memory::{PageAllocator, Region, RegionKind},
    };
    let usable = (0..regions.len())
        .filter_map(|i| regions.get(i))
        .find(|r| {
            r.is_conventional()
                && r.pages >= 64
                && r.base >= 0x100000
                && r.base
                    .checked_add(64 * 4096)
                    .is_some_and(|end| end <= (1u64 << 32))
        })
        .expect("no bootstrap RAM window");
    let mut pages = PageAllocator::<64>::new(
        usable.base,
        &[Region {
            start: usable.base,
            pages: 64,
            kind: RegionKind::Usable,
        }],
    )
    .expect("page allocator");
    let page = pages.allocate().expect("allocate bootstrap page");
    // This conventional memory is identity-mapped by the development firmware.
    // Never reclaim loader code/data, runtime firmware memory, or ACPI regions.
    unsafe {
        let bytes = core::slice::from_raw_parts_mut(page as *mut u8, 4096);
        bytes.fill(0);
        core::ptr::write_volatile(page as *mut u64, 0x48454c4958504147);
        assert_eq!(
            core::ptr::read_volatile(page as *const u64),
            0x48454c4958504147
        );
        bytes.fill(0);
    }
    pages.free(page).expect("free bootstrap page");
    assert_eq!(pages.free_pages(), 64);
    let mut handles = HandleTable::<u64, 4>::new();
    let handle = handles
        .insert(1, Rights::READ.union(Rights::DUPLICATE))
        .expect("handle");
    assert_eq!(
        handles.duplicate(handle, Rights::ADMIN),
        Err(HandleError::Denied)
    );
    let copy = handles
        .duplicate(handle, Rights::READ)
        .expect("attenuated handle");
    assert_eq!(*handles.get(copy, Rights::READ).expect("read"), 1);
    handles.close(handle).expect("close");
    assert_eq!(handles.get(handle, Rights::READ), Err(HandleError::Invalid));
    log!("HELIX: SELFTEST PASS physical page write/zero/free; handle attenuation/stale rejection");
}

#[no_mangle]
pub extern "sysv64" fn helix_fault() -> ! {
    trace::record(
        trace::Category::Fault,
        trace::Severity::Fatal,
        trace::id::KERNEL_PANIC,
        0,
        [0, 0, 0, 0],
    );
    log!("HELIX FATAL: CPU exception; development kernel halted");
    halt()
}
fn halt() -> ! {
    loop {
        input::pump_serial();
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    // The current scheduler is BSP-only and kernel execution has no independent
    // kernel threads yet. Make those limits explicit in the crash record so a
    // future SMP/threaded kernel cannot mistake absent context for CPU 0 data.
    let (file, line, column) = info.location().map_or(("unknown", 0, 0), |location| {
        (location.file(), location.line(), location.column())
    });
    trace::record(
        trace::Category::Fault,
        trace::Severity::Fatal,
        trace::id::KERNEL_PANIC,
        0,
        [0, line as u64, column as u64, 0],
    );
    let _ = panic_report::write_header(
        &mut Serial,
        trace::id::KERNEL_PANIC,
        0,
        "kernel-bsp",
        file,
        line,
        column,
    );
    log!("HELIX PANIC DETAIL: {}", info);
    halt()
}
