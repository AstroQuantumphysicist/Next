//! Bounded single-CPU native process runtime. All mutation runs with IF clear;
//! no Rust reference to Runtime survives an architecture context transition.
#[path = "application.rs"]
mod application;
#[path = "file_namespace_policy.rs"]
mod file_namespace_policy;
#[path = "process_notification.rs"]
mod process_notification;
pub use process_notification::ProcessNotification;
#[path = "service_config.rs"]
mod service_config;
use crate::linux::{self, Executable, LinuxImage, Profile};
use crate::{
    arch::{self, CpuFrame, FaultFrame},
    channel::{Channels, Endpoint, MAX_FRAME, MAX_HANDLES},
    timer,
};
use core::{
    arch::asm,
    fmt::Write,
    mem::MaybeUninit,
    ptr::{addr_of_mut, copy_nonoverlapping},
    sync::atomic::{AtomicBool, Ordering},
};
use helix_core::{
    budget::{Budget, Config as BudgetConfig},
    handle::{Handle, HandleError, HandleTable, Rights},
    scheduler::Scheduler,
};
use next_abi::{
    elf::{PF_R, PF_W, PF_X},
    initrd::InitImage,
    native::{
        NativeImage, StartInfo, IMAGE_BASE, IMAGE_BYTES, IMAGE_PAGES, STACK_BASE, STACK_PAGES,
        STACK_TOP,
    },
    Status,
};
macro_rules! log { ($($a:tt)*) => {{ let _ = writeln!(crate::Serial, $($a)*); }}; }
const PROCESSES: usize = 4;
const NX: u64 = 1 << 63;
// Syscalls run with IF clear on the single development CPU. Large imports use
// bounded static staging rather than exhausting the guarded IRQ stack.
static mut SURFACE_IMPORT: [u8; next_abi::surface::NATIVE_MAX_BUFFER_BYTES] =
    [0; next_abi::surface::NATIVE_MAX_BUFFER_BYTES];
#[repr(C, align(4096))]
struct Space {
    pml4: [u64; 512],
    pdpt: [u64; 512],
    pd: [u64; 512],
    pt: [u64; 512],
    image: [u8; IMAGE_BYTES],
    stack: [u8; STACK_PAGES * 4096],
    anonymous: [u8; linux::ANON_PAGES * 4096],
}
static mut SPACES: [Space; PROCESSES] = [const {
    Space {
        pml4: [0; 512],
        pdpt: [0; 512],
        pd: [0; 512],
        pt: [0; 512],
        image: [0; IMAGE_BYTES],
        stack: [0; STACK_PAGES * 4096],
        anonymous: [0; linux::ANON_PAGES * 4096],
    }
}; PROCESSES];
#[repr(C, align(16))]
struct Fx([u8; 512]);
#[derive(Clone, Copy)]
enum Object {
    // Private placeholder while a syscall reserves capacity before disk mutation.
    Reserved,
    Datagram(helix_core::datagram::SocketId),
    Dns(crate::network_socket::DnsId),
    Surface(crate::native_surface::NativeSurface),
    SurfaceBuffer(crate::native_surface::NativeBuffer),
    Log,
    Launcher,
    // Explicit trusted development-console grant for /native files only.
    // No EXECUTE/DUPLICATE/TRANSFER rights or process/readiness authority.
    FileNamespace,
    Process { slot: usize, generation: u64 },
    Channel(Endpoint),
    File(crate::file::FileHandle),
    Event(crate::event::EventHandle),
}
#[derive(Clone, Copy)]
struct Transfer {
    object: Object,
    rights: Rights,
}
#[derive(Clone, Copy)]
struct PendingWait {
    handle: Handle,
    output: u64,
    deadline: u64,
    writable: bool,
}
struct Process {
    declared_permissions: u64,
    principal: Option<next_package::identity::Principal>,
    pid: u64,
    generation: u64,
    references: u32,
    retired: bool,
    task: Option<Handle>,
    handles: HandleTable<Object, 16>,
    owned: [Option<Handle>; 16],
    frame: CpuFrame,
    fx: Fx,
    flags: [u32; IMAGE_PAGES],
    anonymous: [u8; linux::ANON_PAGES],
    brk_start: u64,
    brk_current: u64,
    fs_base: u64,
    gs_base: u64,
    exit: Option<i64>,
    wake: u64,
    pending_wait: Option<PendingWait>,
    quantum: u64,
    budget: Option<Budget>,
    quota_deadline: u64,
    quota_throttled: bool,
    quota_failed: bool,
    quota_throttles: u64,
    quota_resumes: u64,
    preemptions: u64,
    console_owned: bool,
    profile: Profile,
}
impl Process {
    fn file_authority(&self, raw: u64, write: bool) -> Result<(), Status> {
        let handle = Handle::from_raw(raw);
        let object = self
            .handles
            .get(handle, Rights(0))
            .map_err(|_| Status::AccessDenied)?;
        let role = match object {
            Object::Launcher => file_namespace_policy::Role::Launcher,
            Object::FileNamespace => file_namespace_policy::Role::Namespace,
            _ => file_namespace_policy::Role::Other,
        };
        let available = self
            .handles
            .rights(handle)
            .map_err(|_| Status::AccessDenied)?;
        let (permissions, required) = file_namespace_policy::decision(
            role,
            core_rights_wire(available)?,
            self.declared_permissions,
            write,
        )?;
        if permissions != 0 {
            application::authorize_permissions(
                self.principal.as_ref().ok_or(Status::AccessDenied)?,
                permissions,
                required,
            )
            .map_err(|_| Status::AccessDenied)?;
        }
        Ok(())
    }
    fn network_authority(&self, rights: Rights) -> Result<(), Status> {
        let permission = helix_core::datagram::NETWORK_PERMISSION;
        if self.declared_permissions & permission == 0 {
            return Err(Status::AccessDenied);
        }
        let principal = self.principal.as_ref().ok_or(Status::AccessDenied)?;
        application::authorize_permissions(principal, permission, core_rights_wire(rights)?)
            .map_err(|_| Status::AccessDenied)
    }
    fn portal_identity(&self) -> Result<helix_core::portal::ProcessIdentity, Status> {
        let principal = self.principal.as_ref().ok_or(Status::AccessDenied)?;
        helix_core::portal::ProcessIdentity::new(self.pid, self.generation, principal.identity())
            .map_err(|_| Status::AccessDenied)
    }
    fn insert(&mut self, object: Object, rights: Rights) -> Result<Handle, Status> {
        if matches!(object, Object::Datagram(_) | Object::Dns(_)) {
            self.network_authority(rights)?;
        }
        let principal = self.principal.as_ref().ok_or(Status::AccessDenied)?;
        application::authorize(principal, core_rights_wire(rights)?)
            .map_err(|_| Status::AccessDenied)?;
        let owned_slot = self
            .owned
            .iter()
            .position(Option::is_none)
            .ok_or(Status::NoMemory)?;
        let handle = self.handles.insert(object, rights).map_err(handle_error)?;
        self.owned[owned_slot] = Some(handle);
        Ok(handle)
    }
    fn empty() -> Self {
        Self {
            declared_permissions: 0,
            principal: None,
            pid: 0,
            generation: 1,
            references: 0,
            retired: false,
            task: None,
            handles: HandleTable::new(),
            owned: [None; 16],
            frame: CpuFrame::default(),
            fx: Fx([0; 512]),
            flags: [0; IMAGE_PAGES],
            anonymous: [0; linux::ANON_PAGES],
            brk_start: 0,
            brk_current: 0,
            fs_base: 0,
            gs_base: 0,
            exit: None,
            wake: 0,
            pending_wait: None,
            quantum: 0,
            budget: None,
            quota_deadline: 0,
            quota_throttled: false,
            quota_failed: false,
            quota_throttles: 0,
            quota_resumes: 0,
            preemptions: 0,
            console_owned: false,
            profile: Profile::Native,
        }
    }
}
static QUOTA_AVAILABLE: AtomicBool = AtomicBool::new(false);
static QUOTA_FAILED: AtomicBool = AtomicBool::new(false);
static QUOTA_LAST_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static QUOTA_LAST_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
fn quota_now() -> Option<u64> {
    if !QUOTA_AVAILABLE.load(Ordering::Acquire) || QUOTA_FAILED.load(Ordering::Acquire) {
        return None;
    }
    crate::hpet::accounting_ns()
}
fn quota_clock_irq() {
    if !QUOTA_AVAILABLE.load(Ordering::Acquire) || QUOTA_FAILED.load(Ordering::Acquire) {
        return;
    }
    let tick = timer::ticks();
    let last_tick = QUOTA_LAST_TICK.load(Ordering::Relaxed);
    let previous = QUOTA_LAST_NS.load(Ordering::Relaxed);
    let valid = crate::hpet::accounting_ns()
        .filter(|now| *now >= previous && (tick == last_tick || *now > previous));
    if let Some(now) = valid {
        QUOTA_LAST_NS.store(now, Ordering::Relaxed);
        QUOTA_LAST_TICK.store(tick, Ordering::Relaxed);
    } else if !QUOTA_FAILED.swap(true, Ordering::AcqRel) {
        log!("HELIX: QUOTA CLOCK ERROR stopped/regressed/unavailable HPET; ordinary tasks retained ineligible, protected services keep PIT quanta");
    }
}
fn quota_eligible(process: &mut Process, now: Option<u64>) -> bool {
    if process.quota_failed {
        return false;
    }
    let Some(budget) = &mut process.budget else {
        return true;
    };
    let result = now
        .ok_or(helix_core::budget::Error::Clock)
        .and_then(|now| budget.eligible_at(now));
    match result {
        Ok(eligible) if now.is_some_and(|now| eligible <= now) => {
            if process.quota_throttled {
                process.quota_throttled = false;
                process.quota_resumes = process.quota_resumes.saturating_add(1);
                log!(
                    "HELIX: QUOTA RESUME pid={} count={}",
                    process.pid,
                    process.quota_resumes
                );
            }
            true
        }
        Ok(refill_at) => {
            if !process.quota_throttled {
                process.quota_throttled = true;
                process.quota_throttles = process.quota_throttles.saturating_add(1);
                log!(
                    "HELIX: QUOTA THROTTLE pid={} occupied_ns={} refill_at={} count={}",
                    process.pid,
                    budget.metrics().occupied_ns,
                    refill_at,
                    process.quota_throttles
                );
            }
            false
        }
        Err(error) => {
            process.quota_failed = true;
            log!(
                "HELIX: QUOTA ERROR pid={} eligibility={:?}; task retained ineligible",
                process.pid,
                error
            );
            false
        }
    }
}

fn quota_finish_log(process: &Process) {
    if process.quota_failed {
        log!(
            "HELIX: QUOTA FINISH UNAVAILABLE pid={} accounting_failed=true",
            process.pid
        );
    } else if let Some(budget) = &process.budget {
        let metrics = budget.metrics();
        log!("HELIX: QUOTA FINISH pid={} occupied_ns={} debt_ns={} dispatches={} overruns={} throttles={} resumes={}",
            process.pid,metrics.occupied_ns,budget.debt(),metrics.dispatches,
            metrics.overruns,process.quota_throttles,process.quota_resumes);
    }
}

struct Runtime {
    notifications: process_notification::Queue,
    service_config: Option<service_config::AuthenticatedConfig<'static>>,
    image: InitImage<'static>,
    processes: [Process; PROCESSES],
    scheduler: Scheduler<PROCESSES>,
    channels: Channels<Transfer>,
    active: Option<usize>,
    next_pid: u64,
    ready: bool,
}
static mut RUNTIME: MaybeUninit<Runtime> = MaybeUninit::uninit();
static INITIALIZED: AtomicBool = AtomicBool::new(false);
unsafe fn runtime() -> &'static mut Runtime {
    unsafe { &mut *addr_of_mut!(RUNTIME).cast::<Runtime>() }
}
fn status(s: Status) -> u64 {
    s as i64 as u64
}
fn native_mapping_error(error: u64) -> Status {
    match error {
        linux::ENOMEM => Status::NoMemory,
        linux::EPERM => Status::AccessDenied,
        _ => Status::InvalidArgument,
    }
}
fn handle_error(e: HandleError) -> Status {
    match e {
        HandleError::Full => Status::NoMemory,
        HandleError::Invalid => Status::InvalidHandle,
        HandleError::Denied => Status::AccessDenied,
    }
}
fn wire_rights(raw: u64) -> Result<Rights, Status> {
    let r = next_abi::Rights(raw);
    if !r.is_valid() {
        return Err(Status::InvalidArgument);
    }
    let mut out = 0;
    for (a, b) in [
        (1, 1),
        (2, 2),
        (4, 4),
        (8, 64),
        (16, 128),
        (32, 16),
        (64, 32),
        (128, 512),
    ] {
        if raw & a != 0 {
            out |= b
        }
    }
    Ok(Rights(out))
}
fn core_rights_wire(rights: Rights) -> Result<u64, Status> {
    let mut known = 0;
    let mut wire = 0;
    for (core, abi) in [
        (Rights::READ, next_abi::Rights::READ),
        (Rights::WRITE, next_abi::Rights::WRITE),
        (Rights::EXECUTE, next_abi::Rights::EXECUTE),
        (Rights::DUPLICATE, next_abi::Rights::DUPLICATE),
        (Rights::TRANSFER, next_abi::Rights::TRANSFER),
        (Rights::WAIT, next_abi::Rights::WAIT),
        (Rights::SIGNAL, next_abi::Rights::SIGNAL),
        (Rights::ADMIN, next_abi::Rights::ADMIN),
    ] {
        known |= core.0;
        if rights.contains(core) {
            wire |= abi.0;
        }
    }
    if rights.0 & !known != 0 {
        return Err(Status::AccessDenied);
    }
    Ok(wire)
}
fn user_range(p: &Process, address: u64, len: usize, write: bool) -> bool {
    if len == 0 {
        return (IMAGE_BASE..=STACK_TOP).contains(&address);
    }
    let Some(end) = address.checked_add(len as u64) else {
        return false;
    };
    if address >= STACK_BASE && end <= STACK_TOP {
        return true;
    }
    if address >= linux::ANON_BASE && end <= linux::ANON_END {
        let first = ((address - linux::ANON_BASE) / 4096) as usize;
        let last = ((end - 1 - linux::ANON_BASE) / 4096) as usize;
        return (first..=last).all(|index| {
            p.anonymous[index] & PF_R as u8 != 0 && (!write || p.anonymous[index] & PF_W as u8 != 0)
        });
    }
    if address < IMAGE_BASE || end > IMAGE_BASE + IMAGE_BYTES as u64 {
        return false;
    }
    let first = ((address - IMAGE_BASE) / 4096) as usize;
    let last = ((end - 1 - IMAGE_BASE) / 4096) as usize;
    (first..=last).all(|i| p.flags[i] & PF_R != 0 && (!write || p.flags[i] & PF_W != 0))
}
fn physical_pointer(slot: usize, address: u64) -> *mut u8 {
    unsafe {
        let s = addr_of_mut!(SPACES).cast::<Space>().add(slot);
        if address >= STACK_BASE {
            addr_of_mut!((*s).stack)
                .cast::<u8>()
                .add((address - STACK_BASE) as usize)
        } else if (linux::ANON_BASE..linux::ANON_END).contains(&address) {
            addr_of_mut!((*s).anonymous)
                .cast::<u8>()
                .add((address - linux::ANON_BASE) as usize)
        } else {
            addr_of_mut!((*s).image)
                .cast::<u8>()
                .add((address - IMAGE_BASE) as usize)
        }
    }
}
fn copy_in(rt: &Runtime, slot: usize, address: u64, out: &mut [u8]) -> Result<(), Status> {
    if !user_range(&rt.processes[slot], address, out.len(), false) {
        return Err(Status::InvalidArgument);
    }
    if !out.is_empty() {
        unsafe { copy_nonoverlapping(physical_pointer(slot, address), out.as_mut_ptr(), out.len()) }
    }
    Ok(())
}
impl Runtime {
    fn capture_tls(&mut self, slot: usize) {
        let (fs, gs) = arch::tls_bases();
        self.processes[slot].fs_base = fs;
        self.processes[slot].gs_base = gs;
    }
    fn close(&mut self, owner: usize, handle: Handle) -> Result<(), Status> {
        let p = &mut self.processes[owner];
        let object = p.handles.close(handle).map_err(handle_error)?;
        for h in &mut p.owned {
            if *h == Some(handle) {
                *h = None;
                break;
            }
        }
        self.release_object(object)
    }
    fn close_all(&mut self, owner: usize) {
        let handles = self.processes[owner].owned;
        for handle in handles.into_iter().flatten() {
            let _ = self.close(owner, handle);
        }
    }
    fn retain_object(&mut self, object: Object) -> Result<(), Status> {
        match object {
            Object::Datagram(socket) => crate::network_socket::retain(socket),
            Object::Dns(lookup) => crate::network_socket::dns_retain(lookup),
            Object::Surface(surface) => crate::native_surface::retain_surface(surface),
            Object::SurfaceBuffer(buffer) => crate::native_surface::retain_buffer(buffer),
            Object::Channel(ep) => self.channels.retain(ep),
            Object::File(handle) => crate::file::retain(handle),
            Object::Event(handle) => crate::event::retain(handle),
            Object::Process { slot, generation } => {
                let process = &mut self.processes[slot];
                if process.generation != generation || process.task.is_none() {
                    return Err(Status::InvalidHandle);
                }
                process.references = process.references.checked_add(1).ok_or(Status::NoMemory)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn release_object(&mut self, object: Object) -> Result<(), Status> {
        match object {
            Object::Datagram(socket) => crate::network_socket::close(socket),
            Object::Dns(lookup) => crate::network_socket::dns_close(lookup),
            Object::Surface(surface) => crate::native_surface::close_surface(surface),
            Object::SurfaceBuffer(buffer) => crate::native_surface::close_buffer(buffer),
            Object::Channel(ep) => {
                let discarded = self.channels.close_collect(ep)?;
                for transfer in discarded.into_iter().flatten() {
                    self.release_object(transfer.object)?;
                }
                Ok(())
            }
            Object::File(handle) => crate::file::close(handle),
            Object::Event(handle) => crate::event::close(handle),
            Object::Process { slot, generation } => {
                let process = &mut self.processes[slot];
                if process.generation != generation || process.task.is_none() {
                    return Err(Status::InvalidHandle);
                }
                process.references = process
                    .references
                    .checked_sub(1)
                    .ok_or(Status::InvalidHandle)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn insert_object(
        &mut self,
        owner: usize,
        object: Object,
        rights: Rights,
    ) -> Result<Handle, Status> {
        self.retain_object(object)?;
        match self.processes[owner].insert(object, rights) {
            Ok(handle) => Ok(handle),
            Err(error) => {
                let _ = self.release_object(object);
                Err(error)
            }
        }
    }
    /// Send moves each selected source handle only after enqueue succeeds.
    /// Requested wire rights must be an explicit subset of each TRANSFER cap.
    fn channel_send(
        &mut self,
        owner: usize,
        raw: u64,
        address: u64,
        length: u64,
        rights_address: u64,
    ) -> Result<(), Status> {
        let endpoint = match self.processes[owner]
            .handles
            .get(Handle::from_raw(raw), Rights::WRITE)
            .map_err(handle_error)?
        {
            Object::Channel(endpoint) => *endpoint,
            _ => return Err(Status::InvalidHandle),
        };
        if length > MAX_FRAME as u64 {
            return Err(Status::BufferTooSmall);
        }
        let mut bytes = [0u8; MAX_FRAME];
        let bytes = &mut bytes[..length as usize];
        copy_in(self, owner, address, bytes)?;
        let now = timer::nanoseconds();
        let frame = helix_core::ipc::decode(bytes, 256, MAX_HANDLES, now).map_err(|error| {
            if error == helix_core::ipc::FrameError::Expired {
                Status::TimedOut
            } else {
                Status::InvalidArgument
            }
        })?;
        let count = frame.handle_count();
        let mut rights_bytes = [0u8; MAX_HANDLES * 8];
        if count != 0 {
            copy_in(self, owner, rights_address, &mut rights_bytes[..count * 8])?;
        }
        let mut source = [Handle::from_raw(0); MAX_HANDLES];
        let mut transfers = [Transfer {
            object: Object::Log,
            rights: Rights(0),
        }; MAX_HANDLES];
        for (index, handle) in frame.handles().enumerate() {
            if source[..index].contains(&handle) {
                return Err(Status::InvalidArgument);
            }
            source[index] = handle;
            let rights = wire_rights(u64::from_le_bytes(
                rights_bytes[index * 8..index * 8 + 8]
                    .try_into()
                    .map_err(|_| Status::InvalidArgument)?,
            ))?;
            let table = &self.processes[owner].handles;
            let object = *table.get(handle, Rights::TRANSFER).map_err(handle_error)?;
            if !table.rights(handle).map_err(handle_error)?.contains(rights) {
                return Err(Status::AccessDenied);
            }
            match object {
                Object::Datagram(_) | Object::Dns(_) => {
                    self.processes[owner].network_authority(rights)?
                }
                Object::Launcher | Object::FileNamespace => return Err(Status::AccessDenied),
                // Reject endpoint-reference cycles until an explicit graph policy
                // can prove their queued references are collectable.
                Object::Channel(_) => return Err(Status::NotSupported),
                Object::Surface(_) | Object::SurfaceBuffer(_) => return Err(Status::NotSupported),
                _ => {}
            }
            transfers[index] = Transfer { object, rights };
        }
        for (retained, transfer) in transfers[..count].iter().enumerate() {
            if let Err(error) = self.retain_object(transfer.object) {
                for transfer in &transfers[..retained] {
                    let _ = self.release_object(transfer.object);
                }
                return Err(error);
            }
        }
        if let Err(error) = self
            .channels
            .send_attached(endpoint, bytes, &transfers[..count], now)
        {
            for transfer in &transfers[..count] {
                let _ = self.release_object(transfer.object);
            }
            return Err(error);
        }
        for handle in &source[..count] {
            self.close(owner, *handle)?;
        }
        Ok(())
    }
    fn channel_receive(
        &mut self,
        owner: usize,
        raw: u64,
        address: u64,
        capacity: u64,
    ) -> Result<u64, Status> {
        let endpoint = match self.processes[owner]
            .handles
            .get(Handle::from_raw(raw), Rights::READ)
            .map_err(handle_error)?
        {
            Object::Channel(endpoint) => *endpoint,
            _ => return Err(Status::InvalidHandle),
        };
        let capacity = capacity.min(MAX_FRAME as u64) as usize;
        if !user_range(&self.processes[owner], address, capacity, true) {
            return Err(Status::InvalidArgument);
        }
        let mut bytes = [0u8; MAX_FRAME];
        let (length, transfers) =
            match self
                .channels
                .peek(endpoint, &mut bytes[..capacity], timer::nanoseconds())
            {
                Ok(frame) => frame,
                Err(Status::TimedOut) => {
                    for transfer in self.channels.take(endpoint)?.into_iter().flatten() {
                        self.release_object(transfer.object)?;
                    }
                    return Err(Status::TimedOut);
                }
                Err(error) => return Err(error),
            };
        let mut installed = [None; MAX_HANDLES];
        for (index, transfer) in transfers.into_iter().flatten().enumerate() {
            match self.insert_object(owner, transfer.object, transfer.rights) {
                Ok(handle) => installed[index] = Some(handle),
                Err(error) => {
                    for handle in installed.into_iter().flatten() {
                        let _ = self.close(owner, handle);
                    }
                    return Err(error);
                }
            }
        }
        let payload = u32::from_le_bytes(
            bytes[24..28]
                .try_into()
                .map_err(|_| Status::InvalidArgument)?,
        ) as usize;
        for (index, handle) in installed.into_iter().flatten().enumerate() {
            let offset = helix_core::ipc::HEADER_SIZE + payload + index * 8;
            bytes[offset..offset + 8].copy_from_slice(&handle.raw().to_le_bytes());
        }
        // All writable pages were checked before installation; IF remains clear.
        unsafe {
            copy_nonoverlapping(bytes.as_ptr(), physical_pointer(owner, address), length);
        }
        for transfer in self.channels.take(endpoint)?.into_iter().flatten() {
            self.release_object(transfer.object)?;
        }
        Ok(length as u64)
    }
    /// Validation and consumption share the same masked-interrupt critical
    /// section as registration and signals; no edge can slip between them.
    fn poll_wait(
        &mut self,
        owner: usize,
        handle: Handle,
        output: u64,
        writable: bool,
    ) -> Result<(), Status> {
        let object = *self.processes[owner]
            .handles
            .get(handle, Rights::WAIT)
            .map_err(handle_error)?;
        if !user_range(&self.processes[owner], output, 8, true) {
            return Err(Status::InvalidArgument);
        }
        if writable && !matches!(object, Object::Datagram(_)) {
            return Err(Status::InvalidArgument);
        }
        let code = match object {
            Object::Surface(surface) => {
                if !crate::native_surface::input_ready(surface, self.processes[owner].pid)? {
                    return Err(Status::WouldBlock);
                }
                0
            }
            Object::Dns(lookup) => {
                self.processes[owner].network_authority(Rights::WAIT)?;
                if !crate::network_socket::dns_ready(lookup)? {
                    return Err(Status::WouldBlock);
                }
                0
            }
            Object::Datagram(socket) => {
                self.processes[owner].network_authority(Rights::WAIT)?;
                let ready = if writable {
                    crate::network_socket::write_ready(socket)?
                } else {
                    crate::network_socket::read_ready(socket)?
                };
                if !ready {
                    return Err(Status::WouldBlock);
                }
                0
            }
            Object::Event(event) => {
                crate::event::try_wait(event)?;
                0
            }
            Object::Process { .. } => {
                let target = self.target(owner, handle.raw(), Rights::WAIT)?;
                self.processes[target].exit.ok_or(Status::WouldBlock)?
            }
            _ => return Err(Status::InvalidHandle),
        };
        unsafe {
            copy_nonoverlapping(
                code.to_le_bytes().as_ptr(),
                physical_pointer(owner, output),
                8,
            );
        }
        Ok(())
    }
    fn wake_waiters(&mut self) {
        let now = timer::nanoseconds();
        for owner in 0..PROCESSES {
            let Some(wait) = self.processes[owner].pending_wait else {
                continue;
            };
            let result = if wait.deadline != 0 && now >= wait.deadline {
                Err(Status::TimedOut)
            } else {
                self.poll_wait(owner, wait.handle, wait.output, wait.writable)
            };
            if result == Err(Status::WouldBlock) {
                continue;
            }
            self.processes[owner].pending_wait = None;
            self.processes[owner].frame.rax = result.map(|()| 0).unwrap_or_else(status);
            if let Some(task) = self.processes[owner].task {
                let _ = self.scheduler.wake(task);
            }
        }
    }
    fn collect_exited(&mut self) {
        for slot in 0..PROCESSES {
            let process = &self.processes[slot];
            if process.task.is_some()
                && process.exit.is_some()
                && process.references == 0
                && process.pid != 1
                && self.active != Some(slot)
            {
                let _ = self.reap(slot);
            }
        }
    }
    fn spawn(
        &mut self,
        path: &str,
        root: bool,
        service: Option<next_abi::service::Entry<'static>>,
        files: bool,
    ) -> Result<usize, Status> {
        let bytes = self.image.find(path).ok_or(Status::NotFound)?.data;
        let compat = path.starts_with("/compat/linux/") || path.starts_with("/compat/nt/");
        if files && (root || compat || service.is_some()) {
            return Err(Status::AccessDenied);
        }
        if service.is_some_and(|entry| {
            root || compat || entry.path != path || entry.capability_ceiling & 0x1b != 0x1b
        }) {
            return Err(Status::AccessDenied);
        }
        let ceiling = if let Some(entry) = service {
            entry.capability_ceiling
        } else if compat {
            application::COMPATIBILITY_BRIDGE_RIGHTS
        } else {
            application::NATIVE_BASE_RIGHTS | if root { next_abi::Rights::EXECUTE.0 } else { 0 }
        };
        let loaded = match service {
            Some(entry) => application::load_with_grant(bytes, entry.permissions, ceiling),
            None => application::load(bytes, ceiling),
        }
        .map_err(|error| {
            log!(
                "HELIX: APPLICATION SIGNATURE REJECT path={} reason={:?}",
                path,
                error
            );
            Status::AccessDenied
        })?;
        if files {
            // Authenticate explicit manifest/execution FS grants before creating
            // a PID, task, address space, object or mutable namespace entry.
            application::authorize_permissions(
                &loaded.principal,
                3,
                next_abi::Rights::READ.union(next_abi::Rights::WRITE).0,
            )
            .map_err(|_| Status::AccessDenied)?;
            if loaded.declared_permissions & 3 != 3 {
                return Err(Status::AccessDenied);
            }
        }
        if service.is_some_and(|entry| {
            !service_config::AuthenticatedConfig::worker_matches(entry, &loaded.principal)
        }) {
            return Err(Status::AccessDenied);
        }
        let configuration = if root && path == "/system/nextd.elf" {
            Some(
                service_config::AuthenticatedConfig::load(bytes, &loaded.principal).map_err(
                    |error| {
                        log!("HELIX: SERVICE CONFIG REJECT reason={:?}", error);
                        Status::AccessDenied
                    },
                )?,
            )
        } else {
            None
        };
        let bytes = loaded.payload;
        let executable = if path.starts_with("/compat/linux/") && path.ends_with(".elf") {
            if !arch::syscall_available() {
                return Err(Status::NotSupported);
            }
            Executable::Linux(LinuxImage::parse(bytes).map_err(|_| Status::NotSupported)?)
        } else if path.starts_with("/compat/nt/") && path.ends_with(".exe") {
            Executable::Nt(next_abi::pe::PeImage::parse(bytes).map_err(|error| {
                log!(
                    "HELIX: NT PROFILE image rejected path={} reason={:?}",
                    path,
                    error
                );
                Status::NotSupported
            })?)
        } else {
            Executable::Native(NativeImage::parse(bytes).map_err(|_| Status::InvalidArgument)?)
        };
        if root && executable.profile() != Profile::Native {
            return Err(Status::NotSupported);
        }
        self.collect_exited();
        let slot = self
            .processes
            .iter()
            .position(|p| p.task.is_none() && !p.retired)
            .ok_or(Status::NoMemory)?;
        let pid = self.next_pid;
        let following = pid.checked_add(1).ok_or(Status::NoMemory)?;
        // Only the authenticated boot supervisor and signed service-config
        // workers retain unrestricted development service quanta. A caller's
        // ordinary root/console flag does not confer this scheduling exemption.
        let protected = (pid == 1 && root && configuration.is_some()) || service.is_some();
        let budget = if protected || !QUOTA_AVAILABLE.load(Ordering::Acquire) {
            None
        } else {
            let now = quota_now().ok_or(Status::NotSupported)?;
            Some(
                Budget::identified(
                    BudgetConfig::new(200_000_000, 100_000_000, 20_000_000)
                        .map_err(|_| Status::NotSupported)?,
                    now,
                    pid,
                )
                .map_err(|_| Status::NotSupported)?,
            )
        };
        self.next_pid = following;
        let p = &mut self.processes[slot];
        p.declared_permissions = loaded.declared_permissions;
        p.principal = Some(loaded.principal);
        p.pid = pid;
        p.profile = executable.profile();
        p.anonymous.fill(0);
        p.brk_start = (executable.memory_end() + 4095) & !4095;
        p.brk_current = p.brk_start;
        p.fs_base = 0;
        p.gs_base = 0;
        p.exit = None;
        p.references = 0;
        p.preemptions = 0;
        p.budget = budget;
        p.quota_deadline = 0;
        p.quota_throttled = false;
        p.quota_failed = false;
        p.quota_throttles = 0;
        p.quota_resumes = 0;
        p.console_owned = false;
        p.wake = 0;
        p.pending_wait = None;
        p.handles = HandleTable::new();
        p.owned = [None; 16];
        p.frame = CpuFrame::default();
        p.fx.0.fill(0);
        p.fx.0[0..2].copy_from_slice(&0x37fu16.to_le_bytes());
        p.fx.0[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
        for (i, f) in p.flags.iter_mut().enumerate() {
            *f = executable.page_flags(i)
        }
        let log = if p.profile == Profile::Native {
            p.insert(
                Object::Log,
                Rights::WRITE
                    .union(Rights::DUPLICATE)
                    .union(Rights::TRANSFER),
            )?
            .raw()
        } else {
            0
        };
        let launcher = if files {
            p.insert(Object::FileNamespace, Rights::READ.union(Rights::WRITE))?
                .raw()
        } else if root {
            p.insert(Object::Launcher, Rights::EXECUTE)?.raw()
        } else {
            0
        };
        let startup = StartInfo::new(pid, log, launcher, 0);
        unsafe {
            let space = addr_of_mut!(SPACES).cast::<Space>().add(slot);
            for i in 0..IMAGE_PAGES {
                let a = addr_of_mut!((*space).image).cast::<u8>().add(i * 4096) as u64;
                if !arch::protect_physical(a, true) {
                    return Err(Status::NoMemory);
                }
            }
            executable
                .copy_into(&mut (*space).image)
                .map_err(|_| Status::InvalidArgument)?;
            (*space).stack.fill(0);
            (*space).anonymous.fill(0);
            (*space).pml4.fill(0);
            (*space).pdpt.fill(0);
            (*space).pd.fill(0);
            (*space).pt.fill(0);
            arch::copy_kernel_root(
                addr_of_mut!((*space).pml4).cast(),
                addr_of_mut!((*space).pdpt).cast(),
            );
            (*space).pml4[0] = addr_of_mut!((*space).pdpt) as u64 | 7;
            (*space).pdpt[256] = addr_of_mut!((*space).pd) as u64 | 7;
            (*space).pd[0] = addr_of_mut!((*space).pt) as u64 | 7;
            for i in 0..IMAGE_PAGES {
                let f = p.flags[i];
                if f == 0 {
                    continue;
                }
                let physical = addr_of_mut!((*space).image).cast::<u8>().add(i * 4096) as u64;
                let write = f & PF_W != 0;
                (*space).pt[i] =
                    physical | 5 | if write { 2 } else { 0 } | if f & PF_X == 0 { NX } else { 0 };
                if !arch::protect_physical(physical, write) {
                    return Err(Status::NoMemory);
                }
            }
            for i in 0..STACK_PAGES {
                (*space).pt[256 + i] =
                    addr_of_mut!((*space).stack).cast::<u8>().add(i * 4096) as u64 | 7 | NX
            }
            if p.profile == Profile::Native {
                core::ptr::write_unaligned(
                    addr_of_mut!((*space).stack).cast::<StartInfo>(),
                    startup,
                );
            } else if p.profile == Profile::Linux {
                let path_offset = STACK_PAGES * 4096 - 256;
                (&mut (*space).stack)[path_offset..path_offset + path.len()]
                    .copy_from_slice(path.as_bytes());
                let words = [1u64, STACK_TOP - 256, 0, 0, 6, 4096, 0, 0];
                for (index, word) in words.iter().enumerate() {
                    let offset = STACK_PAGES * 4096 - 512 + index * 8;
                    (&mut (*space).stack)[offset..offset + 8].copy_from_slice(&word.to_le_bytes());
                }
            }
        }
        p.frame.rip = executable.entry();
        p.frame.cs = 35;
        p.frame.ss = 27;
        p.frame.rflags = 0x202;
        p.frame.rsp = if p.profile == Profile::Native {
            STACK_TOP - 8
        } else if p.profile == Profile::Nt {
            // A direct Win64 entry sees a return slot, then 32-byte home space.
            STACK_TOP - 40
        } else {
            STACK_TOP - 512
        };
        p.frame.rdi = if p.profile == Profile::Native {
            STACK_BASE
        } else {
            0
        };
        p.task = Some(self.scheduler.spawn().map_err(|_| Status::NoMemory)?);
        if let Some(configuration) = configuration {
            self.service_config = Some(configuration);
        }
        log!("HELIX: APPLICATION IDENTITY pid={} package={} signer={:02x?} digest={:02x?} devgrant={:#x} cap_ceiling={:#x}",
            pid, loaded.principal.package_id(), &loaded.principal.signer()[..4],
            &loaded.principal.executable_digest()[..4], loaded.principal.permissions(),
            loaded.principal.capability_rights());
        crate::trace::record(
            crate::trace::Category::Process,
            crate::trace::Severity::Info,
            crate::trace::id::PROCESS_CREATED,
            pid,
            [p.profile as u64, 0, 0, 0],
        );
        log!(
            "HELIX: PROCESS SPAWN pid={} slot={} path={} profile={:?}",
            pid,
            slot,
            path,
            p.profile
        );
        Ok(slot)
    }
    fn finish(&mut self, slot: usize, code: i64) {
        if let Ok(identity) = self.processes[slot].portal_identity() {
            crate::kernelportal::process_died(identity);
        }
        crate::linux_fs::close_all(slot, self.processes[slot].generation);
        // Its own held capability protects the target while blocked. Cancel
        // registration before closing that capability during kill/exit.
        self.processes[slot].pending_wait = None;
        self.close_all(slot);
        let p = &mut self.processes[slot];
        if p.exit.is_some() {
            return;
        }
        p.exit = Some(code);
        if let Some(principal) = p.principal {
            if let Some(event) = ProcessNotification::new(
                p.pid,
                principal.package_id(),
                principal.executable_digest(),
                code,
                timer::nanoseconds(),
            ) {
                self.notifications.push(event);
            }
        }
        if let Some(task) = p.task {
            let _ = self.scheduler.exit(task);
        }
        crate::trace::record(
            crate::trace::Category::Process,
            crate::trace::Severity::Info,
            crate::trace::id::PROCESS_EXITED,
            p.pid,
            [p.profile as u64, code as u64, 0, 0],
        );
        log!(
            "HELIX: PROCESS EXIT pid={} status={} preemptions={}",
            p.pid,
            code,
            p.preemptions
        );
        // An inactive console/watchdog kill has no live dispatch ticket. Its
        // completed occupancy is final here; active exits defer until unwind.
        if self.active != Some(slot) {
            quota_finish_log(p);
        }
        self.wake_waiters();
    }
    fn reap(&mut self, slot: usize) -> Result<i64, Status> {
        let p = &mut self.processes[slot];
        let code = p.exit.ok_or(Status::WouldBlock)?;
        if p.references != 0 || self.active == Some(slot) {
            return Err(Status::WouldBlock);
        }
        if let Some(task) = p.task.take() {
            self.scheduler
                .reap(task)
                .map_err(|_| Status::InvalidArgument)?;
        }
        if let Some(generation) = p.generation.checked_add(1) {
            p.generation = generation
        } else {
            p.retired = true;
        }
        p.handles = HandleTable::new();
        p.principal = None;
        p.budget = None;
        p.quota_deadline = 0;
        p.declared_permissions = 0;
        p.pid = 0;
        unsafe {
            let space = addr_of_mut!(SPACES).cast::<Space>().add(slot);
            (*space).pt.fill(0);
            for i in 0..IMAGE_PAGES {
                arch::protect_physical(
                    addr_of_mut!((*space).image).cast::<u8>().add(i * 4096) as u64,
                    true,
                );
            }
            for index in 0..linux::ANON_PAGES {
                arch::protect_physical(
                    addr_of_mut!((*space).anonymous)
                        .cast::<u8>()
                        .add(index * 4096) as u64,
                    true,
                );
            }
            (*space).image.fill(0);
            (*space).stack.fill(0);
            (*space).anonymous.fill(0);
            p.anonymous.fill(0);
        }
        Ok(code)
    }
    fn target(&self, owner: usize, raw: u64, rights: Rights) -> Result<usize, Status> {
        match *self.processes[owner]
            .handles
            .get(Handle::from_raw(raw), rights)
            .map_err(handle_error)?
        {
            Object::Process { slot, generation }
                if self.processes[slot].generation == generation
                    && self.processes[slot].task.is_some() =>
            {
                Ok(slot)
            }
            _ => Err(Status::InvalidHandle),
        }
    }
}
// Construct bounded owners directly in static storage. Keep large channel
// constructor temporaries in a separate frame that is gone before spawn.
#[inline(never)]
unsafe fn initialize_storage(image: InitImage<'static>) {
    let rt = addr_of_mut!(RUNTIME).cast::<Runtime>();
    addr_of_mut!((*rt).notifications).write(process_notification::Queue::new());
    addr_of_mut!((*rt).service_config).write(None);
    addr_of_mut!((*rt).image).write(image);
    for slot in 0..PROCESSES {
        addr_of_mut!((*rt).processes)
            .cast::<Process>()
            .add(slot)
            .write(Process::empty());
    }
    addr_of_mut!((*rt).scheduler).write(Scheduler::new());
    addr_of_mut!((*rt).channels).write(Channels::new());
    addr_of_mut!((*rt).active).write(None);
    addr_of_mut!((*rt).next_pid).write(1);
    addr_of_mut!((*rt).ready).write(false);
}
pub fn initialize(bytes: &'static [u8]) -> Result<(), Status> {
    unsafe {
        asm!("cli", options(nostack));
    }
    if INITIALIZED.load(Ordering::Acquire) {
        return Err(Status::InvalidArgument);
    }
    let image = InitImage::parse(bytes).map_err(|_| Status::InvalidArgument)?;
    unsafe {
        arch::install_native_gates();
        initialize_storage(image);
        if let Some(now) = crate::hpet::accounting_ns() {
            QUOTA_LAST_NS.store(now, Ordering::Relaxed);
            QUOTA_LAST_TICK.store(timer::ticks(), Ordering::Relaxed);
            QUOTA_AVAILABLE.store(true, Ordering::Release);
            log!("HELIX: QUOTA PROFILE HPET64 dispatch occupancy ordinary=100ms/200ms slice<=20ms; PIT delivery overshoot debited; authenticated supervisor/services protected; no RT/SMP/group quotas");
        } else {
            log!("HELIX: QUOTA UNAVAILABLE no validated HPET64 accounting clock; existing PIT quanta retained");
        }
        runtime().spawn("/system/nextd.elf", true, None, false)?;
    }
    INITIALIZED.store(true, Ordering::Release);
    unsafe {
        asm!("sti", options(nostack));
    }
    Ok(())
}
/// Runs at most one bounded quantum. Architecture return restores kernel FX.
pub fn step() -> bool {
    if !INITIALIZED.load(Ordering::Acquire) {
        return false;
    }
    unsafe {
        asm!("cli", options(nostack));
    }
    let selection = unsafe {
        crate::network_socket::pump();
        let rt = runtime();
        rt.wake_waiters();
        rt.collect_exited();
        let now = timer::nanoseconds();
        for p in &mut rt.processes {
            if p.wake != 0 && now >= p.wake {
                p.wake = 0;
                if let Some(task) = p.task {
                    let _ = rt.scheduler.wake(task);
                }
            }
        }
        let budget_now = quota_now();
        let task = {
            let processes = &mut rt.processes;
            rt.scheduler.schedule_eligible(|task| {
                processes
                    .iter_mut()
                    .find(|p| p.task == Some(task))
                    .is_some_and(|p| quota_eligible(p, budget_now))
            })
        };
        task.and_then(|task| rt.processes.iter().position(|p| p.task == Some(task)))
            .and_then(|slot| {
                let ticket = if let Some(budget) = &mut rt.processes[slot].budget {
                    match budget.begin(budget_now?) {
                        Ok(ticket) => Some(ticket),
                        Err(error) => {
                            rt.processes[slot].quota_failed = true;
                            log!(
                                "HELIX: QUOTA ERROR pid={} dispatch={:?}",
                                rt.processes[slot].pid,
                                error
                            );
                            return None;
                        }
                    }
                } else {
                    None
                };
                rt.active = Some(slot);
                rt.processes[slot].quota_deadline = ticket.as_ref().map_or(0, |t| t.deadline());
                rt.processes[slot].quantum = timer::ticks().saturating_add(2);
                let frame = addr_of_mut!(rt.processes[slot].frame);
                let fx = rt.processes[slot].fx.0.as_mut_ptr();
                let space = addr_of_mut!(SPACES).cast::<Space>().add(slot);
                Some((
                    slot,
                    ticket,
                    frame,
                    fx,
                    addr_of_mut!((*space).pml4) as u64,
                    rt.processes[slot].fs_base,
                    rt.processes[slot].gs_base,
                ))
            })
    };
    let ran = if let Some((slot, ticket, frame, fx, cr3, fs, gs)) = selection {
        unsafe {
            arch::activate(cr3);
        }
        unsafe {
            arch::enter_process(frame, fx, fs, gs);
        }
        unsafe {
            arch::activate(arch::kernel_cr3());
        }
        unsafe {
            let rt = runtime();
            // One return path accounts yield, wait, preempt, exit and faults.
            // No budget/runtime reference survives the assembly transition.
            if let Some(ticket) = ticket {
                let process = &mut rt.processes[slot];
                let result = quota_now()
                    .ok_or(helix_core::budget::Error::Clock)
                    .and_then(|now| {
                        process
                            .budget
                            .as_mut()
                            .ok_or(helix_core::budget::Error::Identity)?
                            .finish(ticket, now)
                    });
                match result {
                    Ok(_) => {
                        if process.exit.is_some() {
                            quota_finish_log(process);
                        }
                    }
                    Err(error) => {
                        process.quota_failed = true;
                        log!(
                            "HELIX: QUOTA ERROR pid={} accounting={:?}; task retained ineligible",
                            process.pid,
                            error
                        );
                    }
                }
            }
            rt.processes[slot].quota_deadline = 0;
            rt.active = None;
            rt.collect_exited();
        }
        true
    } else {
        false
    };
    unsafe {
        asm!("sti", options(nostack));
    }
    ran
}
pub fn ready() -> bool {
    if !INITIALIZED.load(Ordering::Acquire) {
        return false;
    }
    unsafe {
        asm!("cli", options(nostack));
        let rt = runtime();
        let result = rt.ready && rt.processes.iter().any(|p| p.pid == 1 && p.exit.is_none());
        asm!("sti", options(nostack));
        result
    }
}
#[no_mangle]
pub extern "sysv64" fn native_dispatch(frame: *mut CpuFrame) -> u64 {
    let f = unsafe { &mut *frame };
    // CPL3 can set NT with POPFQ. Returning that bit through IRETQ would fault
    // inside the kernel instead of remaining a contained user-mode action.
    f.rflags = (f.rflags & 0x25_0dd5) | 0x202;
    let rt = unsafe { runtime() };
    let Some(slot) = rt.active else {
        f.rax = status(Status::NotSupported);
        return 0;
    };
    rt.capture_tls(slot);
    if rt.processes[slot].profile != Profile::Native {
        if rt.processes[slot].profile == Profile::Linux {
            log!(
                "HELIX: LINUX PROFILE native syscall gate rejected number={}",
                f.rax
            );
            f.rax = linux::ENOSYS;
        } else {
            log!(
                "HELIX: NT PROFILE native syscall gate rejected number={}",
                f.rax
            );
            f.rax = status(Status::NotSupported);
        }
        return 0;
    }
    let number = f.rax;
    let result: Result<u64, Status> = match number {
        0 => {
            rt.finish(slot, f.rdi as i64);
            return 1;
        }
        1 => {
            match rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::WRITE)
            {
                Ok(Object::Log) => {
                    let mut bytes = [0u8; 1024];
                    if f.rdx > bytes.len() as u64 {
                        Err(Status::BufferTooSmall)
                    } else {
                        let bytes = &mut bytes[..f.rdx as usize];
                        copy_in(rt, slot, f.rsi, bytes).and_then(|()| {
                            core::str::from_utf8(bytes)
                                .map_err(|_| Status::InvalidArgument)
                                .map(|s| {
                                    log!("NEXT APP pid={}: {}", rt.processes[slot].pid, s);
                                    if rt.processes[slot].console_owned {
                                        crate::console_log::push(rt.processes[slot].pid, s);
                                    }
                                    bytes.len() as u64
                                })
                        })
                    }
                }
                Ok(_) => Err(Status::AccessDenied),
                Err(e) => Err(handle_error(e)),
            }
        }
        2 => rt.close(slot, Handle::from_raw(f.rdi)).map(|()| 0),
        3 => wire_rights(f.rsi).and_then(|rights| {
            let h = Handle::from_raw(f.rdi);
            let p = &rt.processes[slot];
            let object = *p.handles.get(h, Rights::DUPLICATE).map_err(handle_error)?;
            if !p.handles.rights(h).map_err(handle_error)?.contains(rights) {
                return Err(Status::AccessDenied);
            }
            rt.insert_object(slot, object, rights).map(Handle::raw)
        }),
        5 => rt
            .channel_send(slot, f.rdi, f.rsi, f.rdx, f.r10)
            .map(|()| 0),
        6 => rt.channel_receive(slot, f.rdi, f.rsi, f.rdx),
        4 => {
            if f.rdx & !3 != 0 {
                Err(Status::InvalidArgument)
            } else {
                let handle = Handle::from_raw(f.rdi);
                // Validate authority/copyout before parking or deadline outcome.
                let validation = rt.processes[slot]
                    .handles
                    .get(handle, Rights::WAIT)
                    .map_err(handle_error)
                    .and_then(|object| match object {
                        Object::Datagram(_) => rt.processes[slot].network_authority(Rights::WAIT),
                        Object::Dns(_) if f.rdx & 2 == 0 => {
                            rt.processes[slot].network_authority(Rights::WAIT)
                        }
                        Object::Dns(_) => Err(Status::InvalidArgument),
                        Object::Surface(_) if f.rdx & 2 == 0 => Ok(()),
                        Object::Surface(_) => Err(Status::InvalidArgument),
                        Object::Event(_) | Object::Process { .. } if f.rdx & 2 == 0 => Ok(()),
                        Object::Event(_) | Object::Process { .. } => Err(Status::InvalidArgument),
                        _ => Err(Status::InvalidHandle),
                    })
                    .and_then(|()| {
                        if user_range(&rt.processes[slot], f.rsi, 8, true) {
                            Ok(())
                        } else {
                            Err(Status::InvalidArgument)
                        }
                    });
                validation
                    .and_then(|()| {
                        if f.rdx & 1 != 0 && f.r10 != 0 && timer::nanoseconds() >= f.r10 {
                            return Err(Status::TimedOut);
                        }
                        match rt.poll_wait(slot, handle, f.rsi, f.rdx & 2 != 0) {
                            Ok(()) => Ok(0),
                            Err(Status::WouldBlock) if f.rdx & 1 != 0 => {
                                rt.scheduler
                                    .block_current()
                                    .map_err(|_| Status::InvalidArgument)?;
                                if matches!(
                                    rt.processes[slot].handles.get(handle, Rights::WAIT),
                                    Ok(Object::Datagram(_))
                                ) {
                                    log!(
                                        "HELIX: DATAGRAM WAIT parked pid={} direction={}",
                                        rt.processes[slot].pid,
                                        if f.rdx & 2 != 0 { "write" } else { "read" }
                                    );
                                }
                                if matches!(
                                    rt.processes[slot].handles.get(handle, Rights::WAIT),
                                    Ok(Object::Dns(_))
                                ) {
                                    log!("HELIX: DNS WAIT parked pid={}", rt.processes[slot].pid);
                                }
                                if matches!(
                                    rt.processes[slot].handles.get(handle, Rights::WAIT),
                                    Ok(Object::Surface(_))
                                ) {
                                    log!(
                                        "HELIX: CLIENT INPUT WAIT parked pid={}",
                                        rt.processes[slot].pid
                                    );
                                }
                                rt.processes[slot].pending_wait = Some(PendingWait {
                                    handle,
                                    output: f.rsi,
                                    deadline: f.r10,
                                    writable: f.rdx & 2 != 0,
                                });
                                rt.processes[slot].frame = *f;
                                Ok(1)
                            }
                            Err(error) => Err(error),
                        }
                    })
                    .map(|parked| if parked == 1 { 1 } else { 0 })
            }
        }
        7 => {
            if f.rdi != 0 || f.r10 != 0 {
                Err(Status::InvalidArgument)
            } else {
                rt.linux_mmap(slot, 0, f.rsi, f.rdx, 0x22, 0)
                    .map_err(native_mapping_error)
            }
        }
        8 => rt
            .linux_munmap(slot, f.rdi, f.rsi)
            .map_err(native_mapping_error),
        9 => {
            let launcher = rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::EXECUTE)
                .map_err(handle_error)
                .and_then(|o| {
                    if matches!(o, Object::Launcher) {
                        Ok(())
                    } else {
                        Err(Status::AccessDenied)
                    }
                });
            launcher.and_then(|()| {
                if f.r8 & !3 != 0 || f.r8 == 3 || (f.r8 != 2 && f.r9 != 0) {
                    return Err(Status::InvalidArgument);
                }
                let endpoint = if f.r10 == 0 {
                    None
                } else {
                    let h = Handle::from_raw(f.r10);
                    let p = &rt.processes[slot];
                    let object = p.handles.get(h, Rights::TRANSFER).map_err(handle_error)?;
                    let Object::Channel(ep) = object else {
                        return Err(Status::InvalidHandle);
                    };
                    Some((*ep, p.handles.rights(h).map_err(handle_error)?))
                };
                let mut path = [0u8; 64];
                if f.rdx == 0 || f.rdx > path.len() as u64 {
                    return Err(Status::InvalidArgument);
                }
                let path = &mut path[..f.rdx as usize];
                copy_in(rt, slot, f.rsi, path)?;
                let path = core::str::from_utf8(path).map_err(|_| Status::InvalidArgument)?;
                let service = if f.r8 == 2 {
                    let config = rt.service_config.ok_or(Status::AccessDenied)?;
                    let principal = rt.processes[slot]
                        .principal
                        .as_ref()
                        .ok_or(Status::AccessDenied)?;
                    if rt.processes[slot].pid != 1 || !config.recipient_matches(principal) {
                        return Err(Status::AccessDenied);
                    }
                    let entry = config
                        .entry(usize::try_from(f.r9).map_err(|_| Status::InvalidArgument)?)
                        .map_err(|_| Status::AccessDenied)?;
                    if entry.path != path || endpoint.is_none() {
                        return Err(Status::AccessDenied);
                    }
                    let rights = endpoint
                        .map(|(_, rights)| core_rights_wire(rights))
                        .transpose()?
                        .unwrap_or(0);
                    if rights & !entry.capability_ceiling != 0 {
                        return Err(Status::AccessDenied);
                    }
                    Some(entry)
                } else {
                    None
                };
                let child = rt.spawn(path, f.r8 == 1, service, false)?;
                if (endpoint.is_some() || f.r8 == 1)
                    && rt.processes[child].profile != Profile::Native
                {
                    rt.finish(child, Status::Cancelled as i64);
                    let _ = rt.reap(child);
                    return Err(Status::NotSupported);
                }
                if let Some((ep, rights)) = endpoint {
                    if let Err(e) = rt.channels.retain(ep) {
                        rt.finish(child, Status::Cancelled as i64);
                        let _ = rt.reap(child);
                        return Err(e);
                    }
                    let inherited = match rt.processes[child].insert(Object::Channel(ep), rights) {
                        Ok(h) => h.raw(),
                        Err(e) => {
                            let _ = rt.channels.close(ep);
                            rt.finish(child, Status::Cancelled as i64);
                            let _ = rt.reap(child);
                            return Err(e);
                        }
                    };
                    unsafe {
                        let startup = physical_pointer(child, STACK_BASE).cast::<StartInfo>();
                        (*startup).endpoint = inherited;
                    }
                }
                let object = Object::Process {
                    slot: child,
                    generation: rt.processes[child].generation,
                };
                match rt.insert_object(
                    slot,
                    object,
                    Rights::WAIT.union(Rights::SIGNAL).union(Rights::DUPLICATE),
                ) {
                    Ok(h) => Ok(h.raw()),
                    Err(e) => {
                        rt.finish(child, Status::Cancelled as i64);
                        let _ = rt.reap(child);
                        Err(e)
                    }
                }
            })
        }
        10 => {
            f.rax = 0;
            rt.processes[slot].frame = *f;
            return 1;
        }
        11 => Ok(timer::nanoseconds()),
        12 => {
            if !user_range(&rt.processes[slot], f.rdi, 16, true) {
                Err(Status::InvalidArgument)
            } else {
                rt.channels.create().and_then(|[a, b]| {
                    let rights = Rights::READ
                        .union(Rights::WRITE)
                        .union(Rights::TRANSFER)
                        .union(Rights::DUPLICATE);
                    let first = match rt.processes[slot].insert(Object::Channel(a), rights) {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = rt.channels.close(a);
                            let _ = rt.channels.close(b);
                            return Err(e);
                        }
                    };
                    let second = match rt.processes[slot].insert(Object::Channel(b), rights) {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = rt.close(slot, first);
                            let _ = rt.channels.close(b);
                            return Err(e);
                        }
                    };
                    unsafe {
                        let output = physical_pointer(slot, f.rdi);
                        copy_nonoverlapping(first.raw().to_le_bytes().as_ptr(), output, 8);
                        copy_nonoverlapping(second.raw().to_le_bytes().as_ptr(), output.add(8), 8)
                    }
                    Ok(0)
                })
            }
        }
        13 => rt.target(slot, f.rdi, Rights::SIGNAL).map(|target| {
            rt.finish(target, f.rsi as i64);
            0
        }),
        14 => {
            if f.rdi <= timer::nanoseconds() {
                Ok(0)
            } else {
                rt.processes[slot].wake = f.rdi;
                let _ = rt.scheduler.block_current();
                f.rax = 0;
                rt.processes[slot].frame = *f;
                return 1;
            }
        }
        15 => {
            if rt.processes[slot].pid != 1
                || !matches!(
                    rt.processes[slot]
                        .handles
                        .get(Handle::from_raw(f.rdi), Rights::EXECUTE),
                    Ok(Object::Launcher)
                )
            {
                Err(Status::AccessDenied)
            } else {
                rt.ready = true;
                log!("HELIX: NATIVE SYSTEM READY nextd supervisor alive");
                Ok(0)
            }
        }
        16 => {
            let authorized = rt.processes[slot]
                .file_authority(f.rdi, f.r10 != 0 || f.r8 & next_abi::Rights::WRITE.0 != 0);
            if authorized.is_err() {
                Err(Status::AccessDenied)
            } else if f.r8 & !27 != 0
                || f.r10 & !7 != 0
                || f.r10 & 4 != 0 && f.r10 & 1 == 0
                || f.rdx == 0
                || f.rdx > 64
            {
                Err(Status::InvalidArgument)
            } else {
                wire_rights(f.r8).and_then(|rights| {
                    if f.r10 != 0 && !rights.contains(Rights::WRITE) {
                        return Err(Status::AccessDenied);
                    }
                    let mut path = [0u8; 64];
                    let path = &mut path[..f.rdx as usize];
                    copy_in(rt, slot, f.rsi, path)?;
                    let path = core::str::from_utf8(path).map_err(|_| Status::InvalidArgument)?;
                    // Reserve the owner slot before create/truncate can mutate
                    // disk, so handle exhaustion never changes the namespace.
                    let reserved = rt.processes[slot].insert(Object::Reserved, rights)?;
                    let file = match crate::file::open(path, f.r10) {
                        Ok(file) => file,
                        Err(error) => {
                            let _ = rt.close(slot, reserved);
                            return Err(error);
                        }
                    };
                    match rt.processes[slot].handles.get_mut(reserved, Rights(0)) {
                        Ok(object) => {
                            *object = Object::File(file);
                            Ok(reserved.raw())
                        }
                        Err(error) => {
                            let _ = crate::file::close(file);
                            let _ = rt.close(slot, reserved);
                            Err(handle_error(error))
                        }
                    }
                })
            }
        }
        17 => {
            let file = rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::READ)
                .map_err(handle_error)
                .and_then(|object| {
                    if let Object::File(handle) = object {
                        Ok(*handle)
                    } else {
                        Err(Status::InvalidHandle)
                    }
                });
            file.and_then(|file| {
                if f.rdx > 4096 {
                    return Err(Status::BufferTooSmall);
                }
                let capacity = f.rdx as usize;
                if !user_range(&rt.processes[slot], f.rsi, capacity, true) {
                    return Err(Status::InvalidArgument);
                }
                let mut bytes = [0u8; 4096];
                let length = crate::file::read(file, &mut bytes[..capacity])?;
                if length != 0 {
                    unsafe {
                        copy_nonoverlapping(bytes.as_ptr(), physical_pointer(slot, f.rsi), length);
                    }
                }
                Ok(length as u64)
            })
        }
        18 => {
            let file = rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::WRITE)
                .map_err(handle_error)
                .and_then(|object| {
                    if let Object::File(handle) = object {
                        Ok(*handle)
                    } else {
                        Err(Status::InvalidHandle)
                    }
                });
            file.and_then(|file| {
                if f.rdx > 4096 {
                    return Err(Status::BufferTooSmall);
                }
                let mut bytes = [0u8; 4096];
                let bytes = &mut bytes[..f.rdx as usize];
                copy_in(rt, slot, f.rsi, bytes)?;
                crate::file::write(file, bytes).map(|length| length as u64)
            })
        }
        24 | 25 => {
            if rt.processes[slot].file_authority(f.rdi, true).is_err() {
                Err(Status::AccessDenied)
            } else {
                let mut old = [0u8; 32];
                let mut new = [0u8; 32];
                native_path(rt, slot, f.rsi, f.rdx, &mut old).and_then(|old| {
                    if number == 24 {
                        native_path(rt, slot, f.r10, f.r8, &mut new)
                            .and_then(|new| crate::file::rename(old, new))
                            .map(|()| 0)
                    } else {
                        crate::file::remove(old).map(|()| 0)
                    }
                })
            }
        }
        26 => {
            if f.rdi != 0 && !user_range(&rt.processes[slot], f.rdi, 1, false) {
                Err(Status::InvalidArgument)
            } else {
                rt.processes[slot].fs_base = f.rdi;
                unsafe { arch::install_tls_bases(f.rdi, rt.processes[slot].gs_base) }
                Ok(0)
            }
        }
        27 => {
            if !user_range(&rt.processes[slot], f.rdi, 8, true) {
                Err(Status::InvalidArgument)
            } else {
                unsafe {
                    copy_nonoverlapping(
                        rt.processes[slot].fs_base.to_le_bytes().as_ptr(),
                        physical_pointer(slot, f.rdi),
                        8,
                    )
                }
                Ok(0)
            }
        }
        28 => {
            if !user_range(&rt.processes[slot], f.rdi, 16, true) {
                Err(Status::InvalidArgument)
            } else {
                crate::clock::realtime()
                    .map_err(|error| match error {
                        crate::clock::Error::Overflow | crate::clock::Error::Regression => {
                            Status::IoError
                        }
                        _ => Status::NotSupported,
                    })
                    .map(|time| {
                        let mut value = [0u8; 16];
                        value[..8].copy_from_slice(&time.seconds.to_le_bytes());
                        value[8..].copy_from_slice(&time.nanoseconds.to_le_bytes());
                        unsafe {
                            copy_nonoverlapping(value.as_ptr(), physical_pointer(slot, f.rdi), 16)
                        }
                        0
                    })
            }
        }
        29 => (|| {
            let identity = rt.processes[slot].portal_identity()?;
            if !matches!(f.rdi, 1 | 3) {
                return Err(Status::InvalidArgument);
            }
            application::authorize(
                rt.processes[slot]
                    .principal
                    .as_ref()
                    .ok_or(Status::AccessDenied)?,
                f.rdi,
            )
            .map_err(|_| Status::AccessDenied)?;
            crate::kernelportal::request(identity, f.rdi, f.rsi)
        })(),
        30 => (|| {
            let identity = rt.processes[slot].portal_identity()?;
            // Inspect the chosen attenuation, not merely the requested ceiling.
            // The trusted chooser cannot mutate it while this IF-clear handler
            // reserves capacity and consumes the approved open description.
            let rights = crate::kernelportal::approved_rights(identity, f.rdi)?;
            let reserved = rt.processes[slot].insert(Object::Reserved, rights)?;
            let (file, consumed_rights) = match crate::kernelportal::consume(identity, f.rdi) {
                Ok(grant) => grant,
                Err(error) => {
                    let _ = rt.close(slot, reserved);
                    return Err(error);
                }
            };
            if consumed_rights != rights {
                let _ = crate::file::close(file);
                let _ = rt.close(slot, reserved);
                return Err(Status::AccessDenied);
            }
            match rt.processes[slot].handles.get_mut(reserved, Rights(0)) {
                Ok(object) => {
                    *object = Object::File(file);
                    Ok(reserved.raw())
                }
                Err(error) => {
                    let _ = crate::file::close(file);
                    let _ = rt.close(slot, reserved);
                    Err(handle_error(error))
                }
            }
        })(),
        31 => rt.processes[slot]
            .portal_identity()
            .and_then(|identity| crate::kernelportal::cancel(identity, f.rdi).map(|()| 0)),
        32 => (|| {
            let principal = rt.processes[slot]
                .principal
                .as_ref()
                .ok_or(Status::AccessDenied)?;
            if rt.processes[slot].pid != 1 {
                return Err(Status::AccessDenied);
            }
            if f.rsi == 0
                || f.rsi > next_abi::service::MAX_BYTES as u64
                || !user_range(&rt.processes[slot], f.rdi, f.rsi as usize, true)
            {
                return Err(Status::InvalidArgument);
            }
            let config = rt.service_config.ok_or(Status::AccessDenied)?;
            if !config.recipient_matches(principal) {
                return Err(Status::AccessDenied);
            }
            let bytes = config.bytes();
            if f.rsi < bytes.len() as u64 {
                return Err(Status::BufferTooSmall);
            }
            unsafe {
                copy_nonoverlapping(bytes.as_ptr(), physical_pointer(slot, f.rdi), bytes.len())
            }
            Ok(bytes.len() as u64)
        })(),
        33 => (|| {
            if f.rdi > u16::MAX as u64 || f.rsi & !0x3b != 0 {
                return Err(Status::InvalidArgument);
            }
            let rights = wire_rights(f.rsi)?;
            rt.processes[slot].network_authority(rights)?;
            let reserved = rt.processes[slot].insert(Object::Reserved, rights)?;
            let process = &rt.processes[slot];
            let granted = process
                .principal
                .as_ref()
                .ok_or(Status::AccessDenied)?
                .permissions();
            let socket = match crate::network_socket::create(
                f.rdi as u16,
                process.declared_permissions,
                granted,
            ) {
                Ok(socket) => socket,
                Err(error) => {
                    let _ = rt.close(slot, reserved);
                    return Err(error);
                }
            };
            match rt.processes[slot].handles.get_mut(reserved, Rights(0)) {
                Ok(object) => {
                    *object = Object::Datagram(socket);
                    Ok(reserved.raw())
                }
                Err(error) => {
                    let _ = crate::network_socket::close(socket);
                    let _ = rt.close(slot, reserved);
                    Err(handle_error(error))
                }
            }
        })(),
        34 => (|| {
            let socket = match *rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::WRITE)
                .map_err(handle_error)?
            {
                Object::Datagram(socket) => socket,
                _ => return Err(Status::InvalidHandle),
            };
            rt.processes[slot].network_authority(Rights::WRITE)?;
            if f.rdx > helix_core::datagram::MAX_PAYLOAD as u64 {
                return Err(Status::InvalidArgument);
            }
            let mut bytes = [0u8; helix_core::datagram::MAX_PAYLOAD];
            let bytes = &mut bytes[..f.rdx as usize];
            // Empty UDP datagrams access no memory; Rust empty slices may use
            // an arbitrary nonmapped sentinel pointer.
            if !bytes.is_empty() {
                copy_in(rt, slot, f.rsi, bytes)?;
            }
            crate::network_socket::send(socket, bytes).map(|length| length as u64)
        })(),
        35 => (|| {
            let socket = match *rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::READ)
                .map_err(handle_error)?
            {
                Object::Datagram(socket) => socket,
                _ => return Err(Status::InvalidHandle),
            };
            rt.processes[slot].network_authority(Rights::READ)?;
            if f.rdx > helix_core::datagram::MAX_PAYLOAD as u64 {
                return Err(Status::InvalidArgument);
            }
            let capacity = f.rdx as usize;
            if capacity != 0 && !user_range(&rt.processes[slot], f.rsi, capacity, true) {
                return Err(Status::InvalidArgument);
            }
            let mut bytes = [0u8; helix_core::datagram::MAX_PAYLOAD];
            let (length, ticket) =
                crate::network_socket::peek_receive(socket, &mut bytes[..capacity])?;
            if length != 0 {
                unsafe {
                    copy_nonoverlapping(bytes.as_ptr(), physical_pointer(slot, f.rsi), length)
                }
            }
            // The packet remains queued on invalid pointer or short capacity.
            // IF-clear synchronization keeps this exact ticket stable during
            // copyout and the subsequent consume.
            crate::network_socket::consume_receive(socket, ticket)?;
            Ok(length as u64)
        })(),
        36 => (|| {
            use next_abi::dns_lookup::{canonical_name, Family, LOOKUP_RIGHTS, MAX_NAME};
            if f.rsi == 0 || f.rsi > MAX_NAME as u64 || f.r10 & !LOOKUP_RIGHTS != 0 || f.r9 != 0 {
                return Err(Status::InvalidArgument);
            }
            let family = Family::from_wire(f.rdx).ok_or(Status::InvalidArgument)?;
            let rights = wire_rights(f.r10)?;
            rt.processes[slot].network_authority(rights)?;
            let mut name = [0; MAX_NAME];
            let name = &mut name[..f.rsi as usize];
            copy_in(rt, slot, f.rdi, name)?;
            let name = core::str::from_utf8(name).map_err(|_| Status::InvalidArgument)?;
            if !canonical_name(name) {
                return Err(Status::InvalidArgument);
            }
            let deadline = if f.r8 == 0 {
                timer::nanoseconds()
                    .checked_add(5_000_000_000)
                    .ok_or(Status::InvalidArgument)?
            } else {
                f.r8
            };
            let reserved = rt.processes[slot].insert(Object::Reserved, rights)?;
            let process = &rt.processes[slot];
            let granted = process
                .principal
                .as_ref()
                .ok_or(Status::AccessDenied)?
                .permissions();
            let kind = match family {
                Family::A => helix_core::dns::Type::A,
                Family::Aaaa => helix_core::dns::Type::Aaaa,
            };
            let lookup = match crate::network_socket::dns_begin(
                name,
                kind,
                deadline,
                process.declared_permissions,
                granted,
            ) {
                Ok(lookup) => lookup,
                Err(error) => {
                    let _ = rt.close(slot, reserved);
                    return Err(error);
                }
            };
            match rt.processes[slot].handles.get_mut(reserved, Rights(0)) {
                Ok(object) => {
                    *object = Object::Dns(lookup);
                    Ok(reserved.raw())
                }
                Err(error) => {
                    let _ = crate::network_socket::dns_close(lookup);
                    let _ = rt.close(slot, reserved);
                    Err(handle_error(error))
                }
            }
        })(),
        37 => (|| {
            use helix_core::{dns::Address, dns_lookup::Outcome};
            use next_abi::dns_lookup::{Family, LookupResult, CACHE_HIT, RESULT_BYTES};
            let lookup = match *rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::READ)
                .map_err(handle_error)?
            {
                Object::Dns(lookup) => lookup,
                _ => return Err(Status::InvalidHandle),
            };
            rt.processes[slot].network_authority(Rights::READ)?;
            if f.rdx == 0
                || f.rdx > RESULT_BYTES as u64
                || !user_range(&rt.processes[slot], f.rsi, f.rdx as usize, true)
            {
                return Err(Status::InvalidArgument);
            }
            if f.rdx < RESULT_BYTES as u64 {
                return Err(Status::BufferTooSmall);
            }
            let (outcome, cached) = crate::network_socket::dns_result(lookup)?;
            let answer = match outcome {
                Outcome::Answer(answer) => answer,
                Outcome::NxDomain { .. } | Outcome::NoData { .. } => return Err(Status::NotFound),
                Outcome::TimedOut => return Err(Status::TimedOut),
                Outcome::NeedsTcp | Outcome::NeedsAlias(_) | Outcome::Referral => {
                    return Err(Status::NotSupported)
                }
            };
            if answer.count == 0 || answer.count > next_abi::dns_lookup::MAX_ADDRESSES {
                return Err(Status::IoError);
            }
            let family = match answer.addresses[0] {
                Address::V4(_) => Family::A,
                Address::V6(_) => Family::Aaaa,
            };
            let mut result = LookupResult {
                version: 1,
                family: family as u16,
                count: answer.count as u16,
                ttl_seconds: answer.ttl_seconds,
                flags: if cached { CACHE_HIT } else { 0 },
                ..LookupResult::default()
            };
            for (output, address) in result
                .addresses
                .iter_mut()
                .zip(&answer.addresses[..answer.count])
            {
                match (family, address) {
                    (Family::A, Address::V4(bytes)) => output[..4].copy_from_slice(bytes),
                    (Family::Aaaa, Address::V6(bytes)) => *output = *bytes,
                    _ => return Err(Status::IoError),
                }
            }
            let bytes = result.encode().ok_or(Status::IoError)?;
            unsafe {
                copy_nonoverlapping(bytes.as_ptr(), physical_pointer(slot, f.rsi), bytes.len());
            }
            // Query results are immutable and retained until last Close. Repeated
            // reads and duplicates never destroy one another's terminal result.
            Ok(RESULT_BYTES as u64)
        })(),
        38 => (|| {
            use next_abi::surface::{NATIVE_MAX_BUFFER_BYTES, SURFACE_RIGHTS};
            if f.rdi == 0
                || f.rsi == 0
                || f.rdi > 8192
                || f.rsi > 8192
                || f.rdi
                    .checked_mul(f.rsi)
                    .and_then(|area| area.checked_mul(4))
                    .is_none_or(|bytes| bytes > NATIVE_MAX_BUFFER_BYTES as u64)
                || f.rdx & !SURFACE_RIGHTS != 0
                || f.r10 != 0
                || f.r8 != 0
                || f.r9 != 0
            {
                return Err(Status::InvalidArgument);
            }
            let reserved = rt.processes[slot].insert(Object::Reserved, wire_rights(f.rdx)?)?;
            let process = &rt.processes[slot];
            let principal = process.principal.as_ref().ok_or(Status::AccessDenied)?;
            let surface = match crate::native_surface::create(
                process.pid,
                principal,
                process.pid,
                f.rdi as u32,
                f.rsi as u32,
            ) {
                Ok(surface) => surface,
                Err(error) => {
                    let _ = rt.close(slot, reserved);
                    return Err(error);
                }
            };
            match rt.processes[slot].handles.get_mut(reserved, Rights(0)) {
                Ok(object) => {
                    *object = Object::Surface(surface);
                    Ok(reserved.raw())
                }
                Err(error) => {
                    let _ = crate::native_surface::close_surface(surface);
                    let _ = rt.close(slot, reserved);
                    Err(handle_error(error))
                }
            }
        })(),
        39 => (|| {
            use next_abi::surface::{Configure, CONFIGURE_BYTES};
            let surface = match rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::READ)
                .map_err(handle_error)?
            {
                Object::Surface(surface) => *surface,
                _ => return Err(Status::InvalidHandle),
            };
            if f.rdx == 0 || f.rdx > CONFIGURE_BYTES as u64 || f.r10 != 0 || f.r8 != 0 || f.r9 != 0
            {
                return Err(Status::InvalidArgument);
            }
            if !user_range(&rt.processes[slot], f.rsi, f.rdx as usize, true) {
                return Err(Status::InvalidArgument);
            }
            if f.rdx < CONFIGURE_BYTES as u64 {
                return Err(Status::BufferTooSmall);
            }
            let configuration = crate::native_surface::configure(surface, rt.processes[slot].pid)?;
            let geometry = configuration.configure.geometry;
            let bytes = Configure {
                surface_generation: configuration.surface_generation,
                configure_generation: configuration.configure.generation,
                logical_width: geometry.logical_width,
                logical_height: geometry.logical_height,
                pixel_width: geometry.pixel_width,
                pixel_height: geometry.pixel_height,
                scale_16_16: geometry.scale_16_16,
            }
            .encode()
            .map_err(|_| Status::IoError)?;
            unsafe {
                copy_nonoverlapping(bytes.as_ptr(), physical_pointer(slot, f.rsi), bytes.len());
            }
            Ok(CONFIGURE_BYTES as u64)
        })(),
        40 => (|| {
            let surface = match rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::WRITE)
                .map_err(handle_error)?
            {
                Object::Surface(surface) => *surface,
                _ => return Err(Status::InvalidHandle),
            };
            if f.r10 & !next_abi::client_close::FEATURE_CLOSE_REQUEST != 0 || f.r8 != 0 || f.r9 != 0
            {
                return Err(Status::InvalidArgument);
            }
            crate::native_surface::acknowledge_features(
                surface,
                rt.processes[slot].pid,
                f.rsi,
                f.rdx,
                f.r10,
            )?;
            Ok(0)
        })(),
        41 => (|| {
            use next_abi::surface::{BUFFER_RIGHTS, NATIVE_MAX_BUFFER_BYTES};
            if f.rsi == 0
                || f.rsi > NATIVE_MAX_BUFFER_BYTES as u64
                || f.rdx & !BUFFER_RIGHTS != 0
                || f.r10 != 0
                || f.r8 != 0
                || f.r9 != 0
            {
                return Err(Status::InvalidArgument);
            }
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(
                    addr_of_mut!(SURFACE_IMPORT).cast::<u8>(),
                    f.rsi as usize,
                )
            };
            copy_in(rt, slot, f.rdi, bytes)?;
            let reserved = match rt.processes[slot].insert(Object::Reserved, wire_rights(f.rdx)?) {
                Ok(handle) => handle,
                Err(error) => {
                    bytes.fill(0);
                    return Err(error);
                }
            };
            let imported = crate::native_surface::import(rt.processes[slot].pid, bytes);
            bytes.fill(0);
            let buffer = match imported {
                Ok(buffer) => buffer,
                Err(error) => {
                    let _ = rt.close(slot, reserved);
                    return Err(error);
                }
            };
            match rt.processes[slot].handles.get_mut(reserved, Rights(0)) {
                Ok(object) => {
                    *object = Object::SurfaceBuffer(buffer);
                    Ok(reserved.raw())
                }
                Err(error) => {
                    let _ = crate::native_surface::close_buffer(buffer);
                    let _ = rt.close(slot, reserved);
                    Err(handle_error(error))
                }
            }
        })(),
        42 => (|| {
            use next_abi::surface::{Frame, HEADER_BYTES, MAX_BYTES};
            let surface = match rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::WRITE)
                .map_err(handle_error)?
            {
                Object::Surface(surface) => *surface,
                _ => return Err(Status::InvalidHandle),
            };
            if f.rdx < HEADER_BYTES as u64
                || f.rdx > MAX_BYTES as u64
                || f.r10 != 0
                || f.r8 != 0
                || f.r9 != 0
            {
                return Err(Status::InvalidArgument);
            }
            let mut bytes = [0; MAX_BYTES];
            copy_in(rt, slot, f.rsi, &mut bytes[..f.rdx as usize])?;
            let frame =
                Frame::decode(&bytes[..f.rdx as usize]).map_err(|_| Status::InvalidArgument)?;
            // The CPU-copy preview has no capture/redaction policy. Do not
            // silently discard a request for protected presentation.
            if frame.private_content {
                return Err(Status::NotSupported);
            }
            let buffer = match rt.processes[slot]
                .handles
                .get(Handle::from_raw(frame.buffer_handle), Rights::READ)
                .map_err(handle_error)?
            {
                Object::SurfaceBuffer(buffer) => *buffer,
                _ => return Err(Status::InvalidHandle),
            };
            crate::native_surface::publish(surface, rt.processes[slot].pid, frame, buffer)?;
            Ok(0)
        })(),
        43 => (|| {
            use next_abi::client_input::EVENT_BYTES;
            let surface = match rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::READ)
                .map_err(handle_error)?
            {
                Object::Surface(surface) => *surface,
                _ => return Err(Status::InvalidHandle),
            };
            if f.rdx == 0
                || f.rdx > EVENT_BYTES as u64
                || !matches!(f.r10, 0 | 2)
                || f.r8 != 0
                || f.r9 != 0
            {
                return Err(Status::InvalidArgument);
            }
            if !user_range(&rt.processes[slot], f.rsi, f.rdx as usize, true) {
                return Err(Status::InvalidArgument);
            }
            if f.rdx < EVENT_BYTES as u64 {
                return Err(Status::BufferTooSmall);
            }
            let owner = rt.processes[slot].pid;
            let (event, ticket) = crate::native_surface::input_peek_version(surface, owner, f.r10)?;
            let bytes = event.encode().map_err(|_| Status::IoError)?;
            unsafe {
                copy_nonoverlapping(bytes.as_ptr(), physical_pointer(slot, f.rsi), EVENT_BYTES);
            }
            // IF-clear single-CPU dispatch makes peek/copy/consume one bounded
            // transaction; malformed copyout never removes the head event.
            crate::native_surface::input_consume(surface, owner, ticket)?;
            Ok(EVENT_BYTES as u64)
        })(),
        44 => (|| {
            let surface = match rt.processes[slot]
                .handles
                .get(Handle::from_raw(f.rdi), Rights::WRITE)
                .map_err(handle_error)?
            {
                Object::Surface(surface) => *surface,
                _ => return Err(Status::InvalidHandle),
            };
            if !matches!(f.r9, 1 | 2) {
                return Err(Status::InvalidArgument);
            }
            crate::native_surface::close_reply(
                surface,
                rt.processes[slot].pid,
                f.rsi,
                f.rdx,
                f.r10,
                f.r8,
                f.r9,
            )?;
            Ok(0)
        })(),
        19 => rt.processes[slot]
            .handles
            .get(Handle::from_raw(f.rdi), Rights(0))
            .map_err(handle_error)
            .and_then(|object| {
                if let Object::File(handle) = object {
                    crate::file::seek(*handle, f.rsi as i64, f.rdx)
                } else {
                    Err(Status::InvalidHandle)
                }
            }),
        20 => rt.processes[slot]
            .handles
            .get(Handle::from_raw(f.rdi), Rights::READ)
            .map_err(handle_error)
            .and_then(|object| {
                if let Object::File(handle) = object {
                    Ok(*handle)
                } else {
                    Err(Status::InvalidHandle)
                }
            })
            .and_then(|file| {
                if !user_range(&rt.processes[slot], f.rsi, 16, true) {
                    return Err(Status::InvalidArgument);
                }
                let (length, offset) = crate::file::info(file)?;
                let mut bytes = [0u8; 16];
                bytes[..8].copy_from_slice(&(length as u64).to_le_bytes());
                bytes[8..].copy_from_slice(&(offset as u64).to_le_bytes());
                unsafe {
                    copy_nonoverlapping(bytes.as_ptr(), physical_pointer(slot, f.rsi), bytes.len());
                }
                Ok(0)
            }),
        21 => {
            if f.rdi & !3 != 0 || f.rsi & !120 != 0 {
                Err(Status::InvalidArgument)
            } else {
                wire_rights(f.rsi).and_then(|rights| {
                    let reserved = rt.processes[slot].insert(Object::Reserved, rights)?;
                    let event = match crate::event::create(f.rdi & 1 != 0, f.rdi & 2 != 0) {
                        Ok(event) => event,
                        Err(error) => {
                            let _ = rt.close(slot, reserved);
                            return Err(error);
                        }
                    };
                    match rt.processes[slot].handles.get_mut(reserved, Rights(0)) {
                        Ok(object) => {
                            *object = Object::Event(event);
                            Ok(reserved.raw())
                        }
                        Err(error) => {
                            let _ = crate::event::close(event);
                            let _ = rt.close(slot, reserved);
                            Err(handle_error(error))
                        }
                    }
                })
            }
        }
        22 | 23 => rt.processes[slot]
            .handles
            .get(Handle::from_raw(f.rdi), Rights::SIGNAL)
            .map_err(handle_error)
            .and_then(|object| {
                if let Object::Event(event) = object {
                    if number == 22 {
                        crate::event::signal(*event)
                    } else {
                        crate::event::reset(*event)
                    }
                } else {
                    Err(Status::InvalidHandle)
                }
            })
            .map(|()| 0),
        _ => Err(Status::NotSupported),
    };
    if number == 4 && rt.processes[slot].pending_wait.is_some() {
        return 1;
    }
    if number == 22 {
        rt.wake_waiters();
    }
    f.rax = result.unwrap_or_else(status);
    0
}
#[no_mangle]
pub extern "sysv64" fn native_on_timer(frame: *mut CpuFrame) -> u64 {
    if !INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    quota_clock_irq();
    let f = unsafe { &mut *frame };
    if f.cs & 3 != 3 {
        return 0;
    }
    f.rflags = (f.rflags & 0x25_0dd5) | 0x202;
    let rt = unsafe { runtime() };
    let Some(slot) = rt.active else { return 0 };
    rt.capture_tls(slot);
    let budget_expired = rt.processes[slot].quota_deadline != 0
        && quota_now().is_none_or(|now| now >= rt.processes[slot].quota_deadline);
    if timer::ticks() < rt.processes[slot].quantum && !budget_expired {
        return 0;
    }
    rt.processes[slot].frame = *f;
    rt.processes[slot].preemptions = rt.processes[slot].preemptions.saturating_add(1);
    1
}
#[no_mangle]
pub extern "sysv64" fn native_fault(frame: *const FaultFrame, address: u64) -> u64 {
    if !INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    let f = unsafe { &*frame };
    if f.cs & 3 != 3 {
        return 0;
    }
    let rt = unsafe { runtime() };
    let Some(slot) = rt.active else { return 0 };
    let address = if f.vector == 14 { address } else { 0 };
    crate::trace::record(
        crate::trace::Category::Fault,
        crate::trace::Severity::Error,
        crate::trace::id::USER_FAULT,
        rt.processes[slot].pid,
        [rt.processes[slot].profile as u64, f.vector, 0, 0],
    );
    log!(
        "HELIX: PROCESS FAULT pid={} vector={} address={:#x} rip={:#x} error={:#x} contained preemptions={}",
        rt.processes[slot].pid,
        f.vector,
        address,
        f.rip,
        f.error,
        rt.processes[slot].preemptions
    );
    rt.finish(slot, Status::Faulted as i64);
    1
}

/// Console operations are trusted kernel entry points, never native syscalls.
/// They deliberately preserve the caller's interrupt enable state.
struct InterruptGuard(bool);
impl InterruptGuard {
    fn new() -> Self {
        let flags: u64;
        unsafe {
            asm!("pushfq", "pop {}", "cli", out(reg) flags);
        }
        Self(flags & 0x200 != 0)
    }
}
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                asm!("sti", options(nostack));
            }
        }
    }
}
#[derive(Clone, Copy)]
pub struct ProcessSummary {
    pub pid: u64,
    pub state: &'static str,
    pub preemptions: u64,
    pub console_owned: bool,
    package_id: [u8; 64],
    id_len: usize,
    pub signer_prefix: [u8; 4],
    pub digest_prefix: [u8; 4],
    pub developer_grant: u64,
    pub capability_ceiling: u64,
}
/// Trusted kernel consumer; app log messages never populate this queue.
pub fn take_notification() -> Option<ProcessNotification> {
    let _interrupts = InterruptGuard::new();
    if !INITIALIZED.load(Ordering::Acquire) {
        return None;
    }
    unsafe { runtime() }.notifications.pop()
}

pub fn notification_dropped() -> u64 {
    let _interrupts = InterruptGuard::new();
    if !INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    unsafe { runtime() }.notifications.dropped()
}

impl ProcessSummary {
    pub fn package_id(&self) -> &str {
        core::str::from_utf8(&self.package_id[..self.id_len]).unwrap_or("")
    }
}
pub fn summaries() -> [ProcessSummary; PROCESSES] {
    let _interrupts = InterruptGuard::new();
    let mut summaries = [ProcessSummary {
        pid: 0,
        state: "unused",
        preemptions: 0,
        console_owned: false,
        package_id: [0; 64],
        id_len: 0,
        signer_prefix: [0; 4],
        digest_prefix: [0; 4],
        developer_grant: 0,
        capability_ceiling: 0,
    }; PROCESSES];
    if INITIALIZED.load(Ordering::Acquire) {
        let rt = unsafe { runtime() };
        for (summary, p) in summaries.iter_mut().zip(&rt.processes) {
            summary.pid = p.pid;
            summary.preemptions = p.preemptions;
            summary.console_owned = p.console_owned;
            if p.task.is_some() {
                if let Some(principal) = p.principal {
                    let id = principal.package_id();
                    summary.id_len = id.len();
                    summary.package_id[..id.len()].copy_from_slice(id.as_bytes());
                    summary
                        .signer_prefix
                        .copy_from_slice(&principal.signer()[..4]);
                    summary
                        .digest_prefix
                        .copy_from_slice(&principal.executable_digest()[..4]);
                    summary.developer_grant = principal.permissions();
                    summary.capability_ceiling = principal.capability_rights();
                }
            }
            summary.state = if p.task.is_none() {
                "unused"
            } else if p.exit.is_some() {
                "exited"
            } else if p.pending_wait.is_some() {
                "waiting"
            } else if p.wake != 0 {
                "sleeping"
            } else {
                "runnable"
            };
        }
    }
    summaries
}
pub fn console_spawn(path: &str) -> Result<u64, Status> {
    let _interrupts = InterruptGuard::new();
    if !INITIALIZED.load(Ordering::Acquire) {
        return Err(Status::NotSupported);
    }
    let rt = unsafe { runtime() };
    if rt.active.is_some() {
        return Err(Status::AccessDenied);
    }
    let slot = rt.spawn(path, false, None, false)?;
    rt.processes[slot].console_owned = true;
    Ok(rt.processes[slot].pid)
}
/// Trusted development console grants native-file namespace READ|WRITE only.
/// This explicit command is separate from ordinary launch and portal consent.
pub fn console_spawn_with_files(path: &str) -> Result<u64, Status> {
    let _interrupts = InterruptGuard::new();
    if !INITIALIZED.load(Ordering::Acquire) {
        return Err(Status::NotSupported);
    }
    let rt = unsafe { runtime() };
    if rt.active.is_some() {
        return Err(Status::AccessDenied);
    }
    let slot = rt.spawn(path, false, None, true)?;
    rt.processes[slot].console_owned = true;
    log!(
        "HELIX: DEVELOPMENT FILE NAMESPACE GRANT pid={} rights=read,write prefix=/native",
        rt.processes[slot].pid
    );
    Ok(rt.processes[slot].pid)
}
pub fn console_kill(pid: u64) -> Result<(), Status> {
    let _interrupts = InterruptGuard::new();
    if !INITIALIZED.load(Ordering::Acquire) {
        return Err(Status::NotSupported);
    }
    if pid == 1 {
        return Err(Status::AccessDenied);
    }
    let rt = unsafe { runtime() };
    if rt.active.is_some() {
        return Err(Status::AccessDenied);
    }
    let slot = rt
        .processes
        .iter()
        .position(|p| p.pid == pid && p.task.is_some())
        .ok_or(Status::NotFound)?;
    if !rt.processes[slot].console_owned {
        return Err(Status::AccessDenied);
    }
    rt.finish(slot, Status::Cancelled as i64);
    let _ = rt.reap(slot)?;
    Ok(())
}

impl Runtime {
    fn clear_anonymous(&mut self, slot: usize, start: usize, pages: usize) {
        unsafe {
            let space = addr_of_mut!(SPACES).cast::<Space>().add(slot);
            for index in start..start + pages {
                (*space).pt[128 + index] = 0;
                self.processes[slot].anonymous[index] = 0;
                arch::flush_user_page(linux::ANON_BASE + index as u64 * 4096);
                let physical = addr_of_mut!((*space).anonymous)
                    .cast::<u8>()
                    .add(index * 4096);
                if arch::protect_physical(physical as u64, true) {
                    core::ptr::write_bytes(physical, 0, 4096);
                }
            }
        }
    }
    fn linux_mmap(
        &mut self,
        slot: usize,
        address: u64,
        length: u64,
        protection: u64,
        flags: u64,
        offset: u64,
    ) -> Result<u64, u64> {
        if flags != 0x22 || offset != 0 || length == 0 {
            return Err(linux::EINVAL);
        }
        let pages = linux::mapping_pages(length).ok_or(linux::ENOMEM)?;
        let permissions = linux::mapping_flags(protection)?;
        let hint = if (linux::ANON_BASE..linux::ANON_END).contains(&address) {
            ((address - linux::ANON_BASE) / 4096) as usize
        } else {
            0
        };
        let start =
            linux::first_fit(&self.processes[slot].anonymous, pages, hint).ok_or(linux::ENOMEM)?;
        for index in start..start + pages {
            unsafe {
                let space = addr_of_mut!(SPACES).cast::<Space>().add(slot);
                let physical = addr_of_mut!((*space).anonymous)
                    .cast::<u8>()
                    .add(index * 4096);
                if !arch::protect_physical(physical as u64, true) {
                    self.clear_anonymous(slot, start, index - start);
                    return Err(linux::ENOMEM);
                }
                core::ptr::write_bytes(physical, 0, 4096);
                if !arch::protect_physical(physical as u64, permissions & PF_W != 0) {
                    self.clear_anonymous(slot, start, index - start);
                    return Err(linux::ENOMEM);
                }
                let pte = if permissions == 0 {
                    0
                } else {
                    physical as u64
                        | 5
                        | if permissions & PF_W != 0 { 2 } else { 0 }
                        | if permissions & PF_X == 0 { NX } else { 0 }
                };
                (*space).pt[128 + index] = pte;
                self.processes[slot].anonymous[index] = linux::ALLOCATED | permissions as u8;
                arch::flush_user_page(linux::ANON_BASE + index as u64 * 4096);
            }
        }
        Ok(linux::ANON_BASE + start as u64 * 4096)
    }
    fn linux_munmap(&mut self, slot: usize, address: u64, length: u64) -> Result<u64, u64> {
        if !address.is_multiple_of(4096)
            || !(linux::ANON_BASE..linux::ANON_END).contains(&address)
            || length == 0
        {
            return Err(linux::EINVAL);
        }
        let pages = linux::mapping_pages(length).ok_or(linux::EINVAL)?;
        let start = ((address - linux::ANON_BASE) / 4096) as usize;
        if start + pages > linux::ANON_PAGES {
            return Err(linux::EINVAL);
        }
        self.clear_anonymous(slot, start, pages);
        Ok(0)
    }
    fn linux_brk(&mut self, slot: usize, requested: u64) -> u64 {
        let current = self.processes[slot].brk_current;
        if requested < self.processes[slot].brk_start || requested > IMAGE_BASE + IMAGE_BYTES as u64
        {
            return current;
        }
        let old_end = ((current - IMAGE_BASE).div_ceil(4096)) as usize;
        let new_end = ((requested - IMAGE_BASE).div_ceil(4096)) as usize;
        unsafe {
            let space = addr_of_mut!(SPACES).cast::<Space>().add(slot);
            if new_end > old_end {
                for index in old_end..new_end {
                    if self.processes[slot].flags[index] != 0 {
                        return current;
                    }
                }
                for index in old_end..new_end {
                    let physical = addr_of_mut!((*space).image).cast::<u8>().add(index * 4096);
                    core::ptr::write_bytes(physical, 0, 4096);
                    (*space).pt[index] = physical as u64 | 7 | NX;
                    self.processes[slot].flags[index] = PF_R | PF_W;
                    arch::flush_user_page(IMAGE_BASE + index as u64 * 4096);
                }
            } else {
                for index in new_end..old_end {
                    (*space).pt[index] = 0;
                    self.processes[slot].flags[index] = 0;
                    arch::flush_user_page(IMAGE_BASE + index as u64 * 4096);
                    core::ptr::write_bytes(
                        addr_of_mut!((*space).image).cast::<u8>().add(index * 4096),
                        0,
                        4096,
                    );
                }
            }
        }
        self.processes[slot].brk_current = requested;
        requested
    }
}
fn linux_path<'a>(
    rt: &Runtime,
    slot: usize,
    address: u64,
    bytes: &'a mut [u8; 32],
) -> Result<&'a str, u64> {
    for length in 0..bytes.len() {
        let pointer = address.checked_add(length as u64).ok_or(linux::EFAULT)?;
        copy_in(rt, slot, pointer, &mut bytes[length..length + 1]).map_err(|_| linux::EFAULT)?;
        if bytes[length] == 0 {
            let path = core::str::from_utf8(&bytes[..length]).map_err(|_| linux::EINVAL)?;
            // The Linux ABI accepts absolute paths, then the descriptor/VFS
            // adapter returns EACCES for paths outside its granted namespace.
            // Relative path resolution is not implemented in this profile.
            if !path.starts_with('/') {
                return Err(linux::ENOSYS);
            }
            return Ok(path);
        }
    }
    Err((-36i64) as u64)
}
fn valid_native_path(path: &str) -> bool {
    path.starts_with("/native/")
        && path.len() <= 31
        && path[1..].split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}
fn linux_open(rt: &Runtime, slot: usize, address: u64, flags: u64, mode: u64) -> u64 {
    let mut bytes = [0u8; 32];
    linux_path(rt, slot, address, &mut bytes)
        .and_then(|path| {
            crate::linux_fs::open(
                slot,
                rt.processes[slot].generation,
                rt.processes[slot].console_owned,
                path,
                flags,
                mode,
            )
        })
        .unwrap_or_else(|error| error)
}
fn native_path<'a>(
    rt: &Runtime,
    slot: usize,
    address: u64,
    length: u64,
    bytes: &'a mut [u8; 32],
) -> Result<&'a str, Status> {
    if length == 0 || length > 31 {
        return Err(Status::InvalidArgument);
    }
    let out = &mut bytes[..length as usize];
    copy_in(rt, slot, address, out)?;
    let path = core::str::from_utf8(out).map_err(|_| Status::InvalidArgument)?;
    if !valid_native_path(path) {
        return Err(Status::AccessDenied);
    }
    Ok(path)
}
/// SYSCALL always enters with interrupts masked on a trusted kernel stack.
#[no_mangle]
pub extern "sysv64" fn linux_dispatch(frame: *mut CpuFrame) -> u64 {
    let frame = unsafe { &mut *frame };
    // IRET must never inherit user-requested IOPL, NT, VM or reserved bits.
    frame.rflags = (frame.rflags & 0x25_0dd5) | 0x202;
    let rt = unsafe { runtime() };
    let Some(slot) = rt.active else {
        frame.rax = linux::ENOSYS;
        return 0;
    };
    rt.capture_tls(slot);
    if rt.processes[slot].profile != Profile::Linux {
        if rt.processes[slot].profile == Profile::Nt {
            log!(
                "HELIX: NT PROFILE Linux syscall gate rejected number={}",
                frame.rax
            );
        }
        frame.rax = status(Status::NotSupported);
        return 0;
    }
    let generation = rt.processes[slot].generation;
    match frame.rax {
        82 | 264 => {
            let (old_address, new_address) = if frame.rax == 82 {
                (frame.rdi, frame.rsi)
            } else {
                (frame.rsi, frame.r10)
            };
            let mut old = [0u8; 32];
            let mut new = [0u8; 32];
            frame.rax = linux_path(rt, slot, old_address, &mut old)
                .and_then(|old| {
                    linux_path(rt, slot, new_address, &mut new).and_then(|new| {
                        crate::linux_fs::rename(rt.processes[slot].console_owned, old, new)
                            .map(|()| 0)
                    })
                })
                .unwrap_or_else(|error| error);
        }
        87 | 263 => {
            if frame.rax == 263 && frame.rdx != 0 {
                frame.rax = linux::ENOSYS;
                return 0;
            }
            let address = if frame.rax == 87 {
                frame.rdi
            } else {
                frame.rsi
            };
            let mut path = [0u8; 32];
            frame.rax = linux_path(rt, slot, address, &mut path)
                .and_then(|path| {
                    crate::linux_fs::remove(rt.processes[slot].console_owned, path).map(|()| 0)
                })
                .unwrap_or_else(|error| error);
        }
        158 => match frame.rdi {
            0x1001 | 0x1002 => {
                if frame.rsi != 0 && !user_range(&rt.processes[slot], frame.rsi, 1, false) {
                    frame.rax = linux::EINVAL;
                } else {
                    if frame.rdi == 0x1001 {
                        rt.processes[slot].gs_base = frame.rsi;
                    } else {
                        rt.processes[slot].fs_base = frame.rsi;
                    }
                    unsafe {
                        arch::install_tls_bases(
                            rt.processes[slot].fs_base,
                            rt.processes[slot].gs_base,
                        )
                    }
                    frame.rax = 0;
                }
            }
            0x1003 | 0x1004 => {
                if !user_range(&rt.processes[slot], frame.rsi, 8, true) {
                    frame.rax = linux::EFAULT;
                } else {
                    let base = if frame.rdi == 0x1003 {
                        rt.processes[slot].fs_base
                    } else {
                        rt.processes[slot].gs_base
                    };
                    unsafe {
                        copy_nonoverlapping(
                            base.to_le_bytes().as_ptr(),
                            physical_pointer(slot, frame.rsi),
                            8,
                        )
                    }
                    frame.rax = 0;
                }
            }
            _ => frame.rax = linux::EINVAL,
        },
        0 => {
            let size = frame.rdx.min(crate::linux_fs::MAX_IO as u64) as usize;
            if !user_range(&rt.processes[slot], frame.rsi, size, true) {
                frame.rax = linux::EFAULT;
                return 0;
            }
            let mut bytes = [0u8; crate::linux_fs::MAX_IO];
            frame.rax = match crate::linux_fs::read(slot, generation, frame.rdi, &mut bytes[..size])
            {
                Ok(length) => {
                    if length != 0 {
                        unsafe {
                            copy_nonoverlapping(
                                bytes.as_ptr(),
                                physical_pointer(slot, frame.rsi),
                                length,
                            );
                        }
                    }
                    length as u64
                }
                Err(error) => error,
            };
        }
        1 => {
            let stdout = frame.rdi == 1 || frame.rdi == 2;
            if stdout && !crate::linux_fs::stdout_open(slot, generation, frame.rdi) {
                frame.rax = linux::EBADF;
                return 0;
            }
            let size = frame.rdx.min(crate::linux_fs::MAX_IO as u64) as usize;
            let mut buffer = [0u8; crate::linux_fs::MAX_IO];
            match copy_in(rt, slot, frame.rsi, &mut buffer[..size]) {
                Ok(()) if stdout => {
                    linux::write_stdout(&buffer[..size]);
                    if rt.processes[slot].console_owned {
                        if let Ok(text) = core::str::from_utf8(&buffer[..size]) {
                            crate::console_log::push(rt.processes[slot].pid, text);
                        }
                    }
                    frame.rax = size as u64;
                }
                Ok(()) => {
                    frame.rax = crate::linux_fs::write(slot, generation, frame.rdi, &buffer[..size])
                        .map(|length| length as u64)
                        .unwrap_or_else(|error| error)
                }
                Err(_) => frame.rax = linux::EFAULT,
            }
        }
        2 => frame.rax = linux_open(rt, slot, frame.rdi, frame.rsi, frame.rdx),
        257 => frame.rax = linux_open(rt, slot, frame.rsi, frame.rdx, frame.r10),
        3 => {
            frame.rax = crate::linux_fs::close(slot, generation, frame.rdi)
                .map(|()| 0)
                .unwrap_or_else(|error| error)
        }
        5 => {
            if !user_range(
                &rt.processes[slot],
                frame.rsi,
                crate::linux_fs::STAT_BYTES,
                true,
            ) {
                frame.rax = linux::EFAULT;
                return 0;
            }
            let mut bytes = [0u8; crate::linux_fs::STAT_BYTES];
            frame.rax = match crate::linux_fs::stat(slot, generation, frame.rdi, &mut bytes) {
                Ok(()) => {
                    unsafe {
                        copy_nonoverlapping(
                            bytes.as_ptr(),
                            physical_pointer(slot, frame.rsi),
                            bytes.len(),
                        )
                    }
                    0
                }
                Err(error) => error,
            };
        }
        8 => {
            frame.rax =
                crate::linux_fs::seek(slot, generation, frame.rdi, frame.rsi as i64, frame.rdx)
                    .unwrap_or_else(|error| error)
        }
        32 => {
            frame.rax = crate::linux_fs::duplicate(slot, generation, frame.rdi)
                .unwrap_or_else(|error| error)
        }
        228 => {
            if frame.rdi > 1 {
                frame.rax = linux::EINVAL;
                return 0;
            }
            if !user_range(&rt.processes[slot], frame.rsi, 16, true) {
                frame.rax = linux::EFAULT;
                return 0;
            }
            let time = if frame.rdi == 1 {
                crate::clock::monotonic()
            } else {
                match crate::clock::realtime() {
                    Ok(time) => time,
                    Err(error) => {
                        frame.rax = if matches!(
                            error,
                            crate::clock::Error::Overflow | crate::clock::Error::Regression
                        ) {
                            linux::EOVERFLOW
                        } else {
                            linux::ENODEV
                        };
                        return 0;
                    }
                }
            };
            let mut value = [0u8; 16];
            value[..8].copy_from_slice(&time.seconds.to_le_bytes());
            value[8..].copy_from_slice(&time.nanoseconds.to_le_bytes());
            unsafe { copy_nonoverlapping(value.as_ptr(), physical_pointer(slot, frame.rsi), 16) }
            frame.rax = 0;
        }
        35 => {
            let mut input = [0u8; 16];
            if copy_in(rt, slot, frame.rdi, &mut input).is_err() {
                frame.rax = linux::EFAULT;
                return 0;
            }
            if frame.rsi != 0 && !user_range(&rt.processes[slot], frame.rsi, 16, true) {
                frame.rax = linux::EFAULT;
                return 0;
            }
            let seconds = i64::from_le_bytes(input[..8].try_into().unwrap());
            let nanos = i64::from_le_bytes(input[8..].try_into().unwrap());
            if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
                frame.rax = linux::EINVAL;
                return 0;
            }
            let Some(duration) = (seconds as u64)
                .checked_mul(1_000_000_000)
                .and_then(|s| s.checked_add(nanos as u64))
            else {
                frame.rax = linux::EINVAL;
                return 0;
            };
            let Some(deadline) = timer::nanoseconds().checked_add(duration) else {
                frame.rax = linux::EINVAL;
                return 0;
            };
            if frame.rsi != 0 {
                unsafe {
                    core::ptr::write_bytes(physical_pointer(slot, frame.rsi), 0, 16);
                }
            }
            frame.rax = 0;
            if duration != 0 {
                rt.processes[slot].wake = deadline;
                let _ = rt.scheduler.block_current();
                rt.processes[slot].frame = *frame;
                return 1;
            }
        }
        9 => {
            frame.rax = rt
                .linux_mmap(slot, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r9)
                .unwrap_or_else(|errno| errno)
        }
        11 => {
            frame.rax = rt
                .linux_munmap(slot, frame.rdi, frame.rsi)
                .unwrap_or_else(|errno| errno)
        }
        12 => frame.rax = rt.linux_brk(slot, frame.rdi),
        39 => frame.rax = rt.processes[slot].pid,
        24 => {
            frame.rax = 0;
            rt.processes[slot].frame = *frame;
            return 1;
        }
        60 | 231 => {
            rt.finish(slot, (frame.rdi & 255) as i64);
            return 1;
        }
        _ => frame.rax = linux::ENOSYS,
    }
    0
}

/// Original, bounded Win64 API shim gate. These identifiers are private to
/// this compatibility runtime and are not NT system service numbers.
#[no_mangle]
pub extern "sysv64" fn nt_dispatch(frame: *mut CpuFrame) -> u64 {
    let frame = unsafe { &mut *frame };
    frame.rflags = (frame.rflags & 0x25_0dd5) | 0x202;
    let rt = unsafe { runtime() };
    let Some(slot) = rt.active else {
        frame.rax = status(Status::NotSupported);
        return 0;
    };
    rt.capture_tls(slot);
    if rt.processes[slot].profile != Profile::Nt {
        frame.rax = status(Status::NotSupported);
        return 0;
    }
    match frame.rax {
        1 => {
            frame.rax = match frame.rcx as u32 as i32 {
                -11 => (-11i64) as u64,
                -12 => (-12i64) as u64,
                _ => u64::MAX,
            };
        }
        2 => {
            // Only synchronous stdout/stderr writes exist in this profile.
            // Validate every pointer before submitting externally visible data.
            frame.rax = 0;
            if !user_range(&rt.processes[slot], frame.r9, 4, true) {
                return 0;
            }
            unsafe { core::ptr::write_unaligned(physical_pointer(slot, frame.r9).cast::<u32>(), 0) }
            if !matches!(frame.rcx as i64, -11 | -12) || frame.r8 > 1024 {
                return 0;
            }
            let Some(overlapped_address) = frame.rsp.checked_add(40) else {
                return 0;
            };
            let mut overlapped = [0u8; 8];
            if copy_in(rt, slot, overlapped_address, &mut overlapped).is_err()
                || u64::from_le_bytes(overlapped) != 0
            {
                return 0;
            }
            let mut bytes = [0u8; 1024];
            let size = frame.r8 as usize;
            if copy_in(rt, slot, frame.rdx, &mut bytes[..size]).is_err() {
                return 0;
            }
            linux::write_stdout(&bytes[..size]);
            if rt.processes[slot].console_owned {
                if let Ok(text) = core::str::from_utf8(&bytes[..size]) {
                    crate::console_log::push(rt.processes[slot].pid, text);
                }
            }
            unsafe {
                copy_nonoverlapping(
                    (size as u32).to_le_bytes().as_ptr(),
                    physical_pointer(slot, frame.r9),
                    4,
                );
            }
            frame.rax = 1;
        }
        3 => {
            rt.finish(slot, frame.rcx as u32 as i64);
            return 1;
        }
        _ => frame.rax = status(Status::NotSupported),
    }
    0
}
