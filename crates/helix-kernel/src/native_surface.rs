//! Native capability object adapter for owned CPU-copy toplevel surfaces.
//! No displayd isolation, GPU support or implicit capture permission is claimed.
use crate::surface_buffer::{Broker, BufferId, Error, Object, SurfaceId, Ticket};
pub type NativeBuffer = BufferId;
pub type NativeSurface = SurfaceId;
pub type PublicationTicket = Ticket;
use crate::client_input_queue::{Error as InputError, Queue, Record};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use next_abi::client_input::{Event, Key, KeyAction, Payload, RESET_FOCUS_LOSS};
use next_abi::{surface::Frame, Status};
pub type InputTicket = crate::client_input_queue::Ticket;
use prism::surface_state::{Configure, Geometry};
pub const SURFACES: usize = 2;
pub const BUFFERS: usize = 4;
pub const BUFFER_BYTES: usize = 65536;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub pid: u64,
    pub owner: u64,
    pub package: [u8; 64],
    pub package_len: usize,
    pub signer: [u8; 32],
    pub identity: [u8; 32],
}
impl Origin {
    pub fn package_id(&self) -> &str {
        core::str::from_utf8(&self.package[..self.package_len]).unwrap_or("")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub surface_generation: u64,
    pub configure: Configure,
}
pub const CLOSE_TIMEOUT_NS: u64 = 5_000_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosePhase {
    Unsupported,
    Idle,
    Pending,
    Closing,
    Refused,
    Unresponsive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseRequest {
    pub surface_generation: u64,
    pub configure_generation: u64,
    pub ticket: u64,
    pub sequence: u64,
    pub deadline_ns: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseSnapshot {
    pub phase: ClosePhase,
    pub request: Option<CloseRequest>,
}
#[derive(Clone, Copy)]
struct CloseSlot {
    snapshot: CloseSnapshot,
    replied: bool,
    expired: Option<CloseRequest>,
}
impl CloseSlot {
    const fn new() -> Self {
        Self {
            snapshot: CloseSnapshot {
                phase: ClosePhase::Unsupported,
                request: None,
            },
            replied: false,
            expired: None,
        }
    }
    fn negotiate(&mut self, features: u64) {
        *self = Self::new();
        if features & next_abi::client_close::FEATURE_CLOSE_REQUEST != 0 {
            self.snapshot.phase = ClosePhase::Idle;
        }
    }
    fn cancel(&mut self) {
        if self.snapshot.phase != ClosePhase::Unsupported {
            self.snapshot.phase = ClosePhase::Idle;
        }
        self.snapshot.request = None;
        self.replied = false;
        self.expired = None;
    }
    fn expire(&mut self, now: u64) {
        if self
            .snapshot
            .request
            .is_some_and(|request| now >= request.deadline_ns)
        {
            self.expired = self.snapshot.request;
            self.snapshot.request = None;
            self.snapshot.phase = ClosePhase::Unresponsive;
            self.replied = false;
        }
    }
}
pub struct InputRecord {
    record: Record,
    version: u64,
}
impl InputRecord {
    pub fn encode(self) -> Result<[u8; 64], Status> {
        self.record.encode(self.version).map_err(input_status)
    }
}
#[derive(Clone, Copy)]
struct SurfaceOrigin {
    id: SurfaceId,
    origin: Origin,
}
#[derive(Clone, Copy)]
struct BufferOrigin {
    id: BufferId,
    owner: u64,
}
struct Store {
    broker: Broker<SURFACES, BUFFERS, BUFFER_BYTES>,
    surfaces: [Option<SurfaceOrigin>; SURFACES],
    buffers: [Option<BufferOrigin>; BUFFERS],
    input: [Queue; SURFACES],
    input_active: [bool; SURFACES],
    input_time: [u64; SURFACES],
    focused: Option<SurfaceId>,
    closing: [CloseSlot; SURFACES],
    close_counter: u64,
}
impl Store {
    const fn new() -> Self {
        Self {
            broker: Broker::new(),
            surfaces: [None; SURFACES],
            buffers: [None; BUFFERS],
            input: [const { Queue::new() }; SURFACES],
            input_active: [false; SURFACES],
            input_time: [0; SURFACES],
            focused: None,
            closing: [CloseSlot::new(); SURFACES],
            close_counter: 0,
        }
    }
    fn origin(&self, id: SurfaceId) -> Result<Origin, Status> {
        self.surfaces
            .iter()
            .flatten()
            .find(|entry| entry.id == id)
            .map(|entry| entry.origin)
            .ok_or(Status::InvalidHandle)
    }
    fn owner(&self, id: SurfaceId, owner: u64) -> Result<(), Status> {
        if self.origin(id)?.owner == owner {
            Ok(())
        } else {
            Err(Status::AccessDenied)
        }
    }
    fn create(
        &mut self,
        owner: u64,
        origin: Origin,
        width: u32,
        height: u32,
    ) -> Result<SurfaceId, Status> {
        if owner == 0
            || origin.owner != owner
            || origin.pid == 0
            || origin.package_len == 0
            || origin.package_len > 64
        {
            return Err(Status::InvalidArgument);
        }
        if self
            .surfaces
            .iter()
            .flatten()
            .any(|entry| entry.origin.owner == owner)
        {
            return Err(Status::NoMemory);
        }
        let geometry = Geometry {
            logical_width: width,
            logical_height: height,
            pixel_width: width,
            pixel_height: height,
            scale_16_16: 65536,
        };
        geometry.validate().map_err(|_| Status::InvalidArgument)?;
        if u64::from(width) * 4 * u64::from(height) > BUFFER_BYTES as u64 {
            return Err(Status::InvalidArgument);
        }
        let slot = self
            .surfaces
            .iter()
            .position(Option::is_none)
            .ok_or(Status::NoMemory)?;
        let id = self.broker.create_object(owner).map_err(status)?;
        if let Err(error) = self.broker.configure_object(id, geometry) {
            self.broker.release(Object::Surface(id)).map_err(status)?;
            return Err(status(error));
        }
        let generation = self.broker.configuration(id).map_err(status)?.generation;
        self.input[id.slot()]
            .bind(owner, id.slot(), id.generation(), generation)
            .map_err(input_status)?;
        self.input_active[id.slot()] = false;
        self.closing[id.slot()] = CloseSlot::new();
        self.input_time[id.slot()] = 0;
        self.surfaces[slot] = Some(SurfaceOrigin { id, origin });
        Ok(id)
    }
    fn emit(&mut self, id: SurfaceId, payload: Payload, timestamp: u64) -> Result<(), Status> {
        self.origin(id)?;
        let slot = id.slot();
        let configure = self.broker.configuration(id).map_err(status)?.generation;
        let Some(sequence) = self.input[slot].last_sequence().checked_add(1) else {
            self.focused = None;
            self.input[slot].exhaust();
            self.closing[slot].cancel();
            return Err(Status::NoMemory);
        };
        let result = self.input[slot].enqueue(Event {
            surface_generation: id.generation(),
            configure_generation: configure,
            sequence,
            timestamp_ns: timestamp,
            payload,
        });
        if matches!(result, Err(InputError::Overflow | InputError::Exhausted)) {
            self.focused = None;
            self.closing[slot].cancel();
        }
        if result.is_ok() || matches!(result, Err(InputError::Overflow | InputError::Exhausted)) {
            self.input_time[slot] = timestamp;
        }
        result.map_err(input_status)
    }
    fn suspend(&mut self, timestamp: u64) -> Result<(), Status> {
        let Some(id) = self.focused else {
            return Ok(());
        };
        if timestamp < self.input_time[id.slot()] {
            return Err(Status::InvalidArgument);
        }
        self.focused = None;
        self.emit(
            id,
            Payload::Reset {
                reason: RESET_FOCUS_LOSS,
            },
            timestamp,
        )?;
        self.emit(id, Payload::FocusLoss, timestamp)
    }
    fn focus(&mut self, id: SurfaceId, timestamp: u64) -> Result<(), Status> {
        self.origin(id)?;
        if !self.input_active[id.slot()] {
            return Err(Status::WouldBlock);
        }
        if self.focused == Some(id) {
            return Ok(());
        }
        self.suspend(timestamp)?;
        self.emit(id, Payload::FocusGain, timestamp)?;
        self.focused = Some(id);
        Ok(())
    }
    /// Prepared transaction: no frame/input/focus/close mutation on failure.
    fn reconfigure_trusted(
        &mut self,
        id: SurfaceId,
        expected_configuration: u64,
        geometry: Geometry,
        timestamp: u64,
    ) -> Result<Configuration, Status> {
        self.origin(id)?;
        let slot = id.slot();
        if self.broker.configuration(id).map_err(status)?.generation != expected_configuration {
            return Err(Status::InvalidArgument);
        }
        if timestamp < self.input_time[slot] {
            return Err(Status::InvalidArgument);
        }
        let next = self
            .broker
            .next_configuration(id, geometry)
            .map_err(status)?;
        let was_focused = self.focused == Some(id);
        let mut queue = self.input[slot];
        queue.reconfigure(next.generation).map_err(input_status)?;
        for payload in [
            Some(Payload::Reset {
                reason: next_abi::client_input::RESET_RECONFIGURE,
            }),
            was_focused.then_some(Payload::FocusLoss),
        ]
        .into_iter()
        .flatten()
        {
            let sequence = queue
                .last_sequence()
                .checked_add(1)
                .ok_or(Status::NoMemory)?;
            queue
                .enqueue(Event {
                    surface_generation: id.generation(),
                    configure_generation: next.generation,
                    sequence,
                    timestamp_ns: timestamp,
                    payload,
                })
                .map_err(input_status)?;
        }
        // The copied queue and state have passed every fallible check. The BSP
        // guard prevents a different configure between preflight and commit.
        self.broker.configure_object(id, geometry).map_err(status)?;
        self.input[slot] = queue;
        self.closing[slot] = CloseSlot::new();
        self.input_active[slot] = false;
        self.input_time[slot] = timestamp;
        if was_focused {
            self.focused = None;
        }
        Ok(Configuration {
            surface_generation: id.generation(),
            configure: next,
        })
    }
    #[cfg(test)]
    fn input_configuration(
        &mut self,
        id: SurfaceId,
        geometry: Geometry,
        timestamp: u64,
    ) -> Result<(), Status> {
        let expected = self.broker.configuration(id).map_err(status)?.generation;
        self.reconfigure_trusted(id, expected, geometry, timestamp)
            .map(|_| ())
    }
    fn acknowledge_features(
        &mut self,
        id: SurfaceId,
        owner: u64,
        surface_generation: u64,
        configure_generation: u64,
        features: u64,
    ) -> Result<(), Status> {
        if features & !next_abi::client_close::FEATURE_CLOSE_REQUEST != 0 {
            return Err(Status::InvalidArgument);
        }
        self.owner(id, owner)?;
        if surface_generation != id.generation() {
            return Err(Status::InvalidArgument);
        }
        self.broker
            .acknowledge_object(id, owner, configure_generation)
            .map_err(status)?;
        if self.input[id.slot()].configure_generation() != configure_generation {
            self.input[id.slot()]
                .reconfigure(configure_generation)
                .map_err(input_status)?;
            let was_focused = self.focused == Some(id);
            if was_focused {
                self.focused = None;
            }
            let timestamp = self.input_time[id.slot()];
            self.emit(
                id,
                Payload::Reset {
                    reason: next_abi::client_input::RESET_RECONFIGURE,
                },
                timestamp,
            )?;
            if was_focused {
                self.emit(id, Payload::FocusLoss, timestamp)?;
            }
        }
        self.closing[id.slot()].negotiate(features);
        self.input_active[id.slot()] = true;
        Ok(())
    }
    fn request_close(&mut self, id: SurfaceId, now: u64) -> Result<CloseSnapshot, Status> {
        self.origin(id)?;
        let slot = id.slot();
        self.closing[slot].expire(now);
        if self.closing[slot].snapshot.phase == ClosePhase::Unsupported {
            return Err(Status::NotSupported);
        }
        if !self.input_active[slot] {
            return Err(Status::WouldBlock);
        }
        if self.closing[slot].snapshot.request.is_some() {
            return Err(Status::WouldBlock);
        }
        let ticket = self.close_counter.checked_add(1).ok_or(Status::NoMemory)?;
        let Some(sequence) = self.input[slot].last_sequence().checked_add(1) else {
            self.focused = None;
            self.input[slot].exhaust();
            self.closing[slot].cancel();
            return Err(Status::NoMemory);
        };
        let deadline_ns = now
            .checked_add(CLOSE_TIMEOUT_NS)
            .ok_or(Status::InvalidArgument)?;
        let configure_generation = self.broker.configuration(id).map_err(status)?.generation;
        let event = next_abi::client_close::Event {
            surface_generation: id.generation(),
            configure_generation,
            sequence,
            timestamp_ns: now,
            payload: next_abi::client_close::Payload::CloseRequested { ticket },
        };
        let result = self.input[slot].enqueue_record(Record::Close(event));
        if matches!(result, Err(InputError::Overflow | InputError::Exhausted)) {
            self.focused = None;
            self.closing[slot].cancel();
            self.input_time[slot] = now;
        }
        result.map_err(input_status)?;
        self.close_counter = ticket;
        self.input_time[slot] = now;
        self.closing[slot] = CloseSlot {
            snapshot: CloseSnapshot {
                phase: ClosePhase::Pending,
                request: Some(CloseRequest {
                    surface_generation: id.generation(),
                    configure_generation,
                    ticket,
                    sequence,
                    deadline_ns,
                }),
            },
            replied: false,
            expired: None,
        };
        Ok(self.closing[slot].snapshot)
    }
    fn close_reply(
        &mut self,
        id: SurfaceId,
        owner: u64,
        identity: next_abi::client_close::Request,
        response: u64,
        now: u64,
    ) -> Result<(), Status> {
        if !matches!(response, 1 | 2) {
            return Err(Status::InvalidArgument);
        }
        self.owner(id, owner)?;
        let slot = id.slot();
        self.closing[slot].expire(now);
        let matches = |request: CloseRequest| {
            request.surface_generation == identity.surface_generation
                && request.configure_generation == identity.configure_generation
                && request.ticket == identity.ticket
                && request.sequence == identity.sequence
        };
        let Some(request) = self.closing[slot].snapshot.request else {
            return Err(if self.closing[slot].expired.is_some_and(matches) {
                Status::TimedOut
            } else {
                Status::InvalidHandle
            });
        };
        if self.closing[slot].replied || !matches(request) {
            return Err(Status::InvalidHandle);
        }
        self.closing[slot].replied = true;
        self.closing[slot].snapshot.phase = if response == 1 {
            ClosePhase::Refused
        } else {
            ClosePhase::Closing
        };
        if response == 1 {
            self.closing[slot].snapshot.request = None;
        }
        Ok(())
    }
    fn import(&mut self, owner: u64, bytes: &[u8]) -> Result<BufferId, Status> {
        if self
            .buffers
            .iter()
            .flatten()
            .filter(|entry| entry.owner == owner)
            .count()
            >= 2
        {
            return Err(Status::NoMemory);
        }
        let slot = self
            .buffers
            .iter()
            .position(Option::is_none)
            .ok_or(Status::NoMemory)?;
        let id = self.broker.import_object(owner, bytes).map_err(status)?;
        self.buffers[slot] = Some(BufferOrigin { id, owner });
        Ok(id)
    }
    fn peek(
        &self,
        slot: usize,
        output: &mut [u8],
    ) -> Result<Option<(SurfaceId, Frame, Origin, Ticket)>, Status> {
        let Some(id) = self.broker.pending_at(slot) else {
            return Ok(None);
        };
        let origin = self.origin(id)?;
        let (frame, ticket) = self.broker.peek_object(id, output).map_err(status)?;
        Ok(Some((id, frame, origin, ticket)))
    }
    fn consume(&mut self, id: SurfaceId, ticket: Ticket) -> Result<(), Status> {
        self.broker.consume_object(id, ticket).map_err(status)
    }
    fn origins(&self) -> [Option<(SurfaceId, Origin)>; SURFACES] {
        self.surfaces
            .map(|entry| entry.map(|entry| (entry.id, entry.origin)))
    }
    fn close(&mut self, object: Object) -> Result<(), Status> {
        self.broker.release(object).map_err(status)?;
        if self.broker.references(object).is_err() {
            match object {
                Object::Surface(id) => {
                    self.input[id.slot()].clear();
                    self.closing[id.slot()] = CloseSlot::new();
                    self.input_active[id.slot()] = false;
                    self.input_time[id.slot()] = 0;
                    if self.focused == Some(id) {
                        self.focused = None;
                    }
                    if let Some(entry) = self
                        .surfaces
                        .iter_mut()
                        .find(|entry| entry.is_some_and(|entry| entry.id == id))
                    {
                        *entry = None;
                    }
                }
                Object::Buffer(id) => {
                    if let Some(entry) = self
                        .buffers
                        .iter_mut()
                        .find(|entry| entry.is_some_and(|entry| entry.id == id))
                    {
                        *entry = None;
                    }
                }
            }
        }
        Ok(())
    }
}
struct Shared(UnsafeCell<Store>);
unsafe impl Sync for Shared {}
static STORE: Shared = Shared(UnsafeCell::new(Store::new()));
static LOCK: AtomicBool = AtomicBool::new(false);
struct Guard;
impl Guard {
    fn acquire() -> Result<Self, Status> {
        LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| Status::WouldBlock)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}
fn status(error: Error) -> Status {
    match error {
        Error::InvalidHandle => Status::InvalidHandle,
        #[cfg(test)]
        Error::Denied => Status::AccessDenied,
        Error::Full | Error::References => Status::NoMemory,
        Error::WouldBlock => Status::WouldBlock,
        Error::State(prism::surface_state::Error::Owner) => Status::AccessDenied,
        Error::State(prism::surface_state::Error::Exhausted) => Status::NoMemory,
        Error::State(prism::surface_state::Error::Unacknowledged) => Status::WouldBlock,
        _ => Status::InvalidArgument,
    }
}
/// Runtime supplies the authenticated immutable principal; no app-selected
/// title/package/security flag is accepted as origin identity.
pub fn create(
    owner: u64,
    principal: &next_package::identity::Principal,
    pid: u64,
    width: u32,
    height: u32,
) -> Result<SurfaceId, Status> {
    let package = principal.package_id().as_bytes();
    if package.is_empty() || package.len() > 64 {
        return Err(Status::InvalidArgument);
    }
    let mut origin = Origin {
        pid,
        owner,
        package: [0; 64],
        package_len: package.len(),
        signer: principal.signer(),
        identity: principal.identity(),
    };
    origin.package[..package.len()].copy_from_slice(package);
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.create(owner, origin, width, height)
}
/// Trusted resize-release only: the exact preview generation must still be
/// current. Failure leaves all publication/input/focus/close state unchanged.
/// Success retires the old publication; the consumer must retire content while
/// preserving the live surface identity and presentation frame watermark.
pub fn reconfigure_trusted(
    id: SurfaceId,
    expected_configuration: u64,
    geometry: Geometry,
    timestamp: u64,
) -> Result<Configuration, Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.reconfigure_trusted(
        id,
        expected_configuration,
        geometry,
        timestamp,
    )
}
pub fn configure(id: SurfaceId, owner: u64) -> Result<Configuration, Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &*STORE.0.get() };
    store.owner(id, owner)?;
    Ok(Configuration {
        surface_generation: id.generation(),
        configure: store.broker.configuration(id).map_err(status)?,
    })
}
pub fn acknowledge_features(
    id: SurfaceId,
    owner: u64,
    surface_generation: u64,
    configure_generation: u64,
    features: u64,
) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.acknowledge_features(
        id,
        owner,
        surface_generation,
        configure_generation,
        features,
    )
}
pub fn import(owner: u64, bytes: &[u8]) -> Result<BufferId, Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.import(owner, bytes)
}
pub fn publish(id: SurfaceId, owner: u64, frame: Frame, buffer: BufferId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    store.owner(id, owner)?;
    store
        .broker
        .publish_objects(id, owner, frame, buffer)
        .map(|_| ())
        .map_err(status)
}
pub fn retain_surface(id: SurfaceId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }
        .broker
        .retain(Object::Surface(id))
        .map_err(status)
}
pub fn retain_buffer(id: BufferId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }
        .broker
        .retain(Object::Buffer(id))
        .map_err(status)
}
pub fn close_surface(id: SurfaceId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.close(Object::Surface(id))
}
pub fn close_buffer(id: BufferId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.close(Object::Buffer(id))
}
/// Trusted main compositor copies pixels into its independent staging storage.
/// Consumption follows actual UI acceptance; a rejected staging operation keeps
/// publication pending and preserves producer backpressure for retry.
pub fn peek(
    slot: usize,
    output: &mut [u8],
) -> Result<Option<(SurfaceId, Frame, Origin, Ticket)>, Status> {
    let _guard = Guard::acquire()?;
    unsafe { &*STORE.0.get() }.peek(slot, output)
}
/// Exact publication fence before render/present, including configuration.
/// A transient lock failure does not authorize showing the staged pixels.
pub fn publication_is_current(id: SurfaceId, ticket: Ticket) -> Result<bool, Status> {
    let _guard = Guard::acquire()?;
    unsafe { &*STORE.0.get() }
        .broker
        .publication_is_current(id, ticket)
        .map_err(status)
}
pub fn consume(id: SurfaceId, ticket: Ticket) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.consume(id, ticket)
}
/// Trusted lifecycle snapshot for removing previously staged windows. Object
/// generations prevent a newly reused broker slot from inheriting old pixels.
pub fn origins() -> Result<[Option<(SurfaceId, Origin)>; SURFACES], Status> {
    let _guard = Guard::acquire()?;
    Ok(unsafe { &*STORE.0.get() }.origins())
}
pub fn is_live(id: SurfaceId) -> Result<bool, Status> {
    let _guard = Guard::acquire()?;
    Ok(unsafe { &*STORE.0.get() }.origin(id).is_ok())
}
fn input_status(error: InputError) -> Status {
    match error {
        InputError::Empty | InputError::Overflow => Status::WouldBlock,
        InputError::Unbound => Status::InvalidHandle,
        InputError::Exhausted => Status::NoMemory,
        InputError::Invalid | InputError::Stale => Status::InvalidArgument,
        InputError::Unsupported => Status::NotSupported,
    }
}
pub fn input_ready(id: SurfaceId, owner: u64) -> Result<bool, Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &*STORE.0.get() };
    store.owner(id, owner)?;
    Ok(store.input[id.slot()].ready())
}
#[cfg(test)]
pub fn input_peek(id: SurfaceId, owner: u64) -> Result<(Event, InputTicket), Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &*STORE.0.get() };
    store.owner(id, owner)?;
    store.input[id.slot()].peek().map_err(input_status)
}
pub fn input_consume(id: SurfaceId, owner: u64, ticket: InputTicket) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    store.owner(id, owner)?;
    store.input[id.slot()].consume(ticket).map_err(input_status)
}
pub fn input_peek_version(
    id: SurfaceId,
    owner: u64,
    version: u64,
) -> Result<(InputRecord, InputTicket), Status> {
    if !matches!(version, 0 | 2) {
        return Err(Status::InvalidArgument);
    }
    let _guard = Guard::acquire()?;
    let store = unsafe { &*STORE.0.get() };
    store.owner(id, owner)?;
    let (record, ticket) = if version == 0 {
        let (event, ticket) = store.input[id.slot()].peek().map_err(input_status)?;
        (Record::Input(event), ticket)
    } else {
        store.input[id.slot()].peek_record().map_err(input_status)?
    };
    record.encode(version).map_err(input_status)?;
    Ok((InputRecord { record, version }, ticket))
}
/// Trusted compositor gesture only; a request never releases an owner's cap.
pub fn request_close(id: SurfaceId, timestamp: u64) -> Result<CloseSnapshot, Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.request_close(id, timestamp)
}
pub fn close_snapshot(id: SurfaceId, timestamp: u64) -> Result<CloseSnapshot, Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    store.origin(id)?;
    store.closing[id.slot()].expire(timestamp);
    Ok(store.closing[id.slot()].snapshot)
}
pub fn close_reply(
    id: SurfaceId,
    owner: u64,
    surface_generation: u64,
    configure_generation: u64,
    ticket: u64,
    sequence: u64,
    response: u64,
) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.close_reply(
        id,
        owner,
        next_abi::client_close::Request {
            surface_generation,
            configure_generation,
            ticket,
            sequence,
        },
        response,
        crate::timer::nanoseconds(),
    )
}
/// Trusted compositor state query; a stale generation never reports new focus.
pub fn input_is_focused(id: SurfaceId) -> Result<bool, Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &*STORE.0.get() };
    store.origin(id)?;
    Ok(store.focused == Some(id))
}
/// Trusted compositor only: call after a real eligible client-content gesture.
pub fn focus_gesture(id: SurfaceId, timestamp: u64) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.focus(id, timestamp)
}
/// Trusted compositor only: revoke client focus before modal/chrome routing.
pub fn suspend_input(timestamp: u64) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.suspend(timestamp)
}
/// True means the key was queued for the focused client. No focus is acquired.
pub fn route_key(
    id: SurfaceId,
    key: Key,
    action: KeyAction,
    timestamp: u64,
) -> Result<bool, Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    store.origin(id)?;
    if store.focused != Some(id) {
        return Ok(false);
    }
    store.emit(id, Payload::Key { key, action }, timestamp)?;
    Ok(true)
}
/// Coordinates are client-content pixels; only current gesture focus receives them.
pub fn pointer(id: SurfaceId, x: i32, y: i32, buttons: u32, timestamp: u64) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    store.origin(id)?;
    if store.focused != Some(id) {
        return Err(Status::AccessDenied);
    }
    let geometry = store.broker.configuration(id).map_err(status)?.geometry;
    if x < 0 || y < 0 || x as u32 >= geometry.pixel_width || y as u32 >= geometry.pixel_height {
        return Err(Status::InvalidArgument);
    }
    store.emit(id, Payload::Pointer { x, y, buttons }, timestamp)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn origin(owner: u64) -> Origin {
        let mut package = [0; 64];
        package[..11].copy_from_slice(b"org.next.ui");
        Origin {
            owner,
            pid: owner,
            package,
            package_len: 11,
            signer: [1; 32],
            identity: [2; 32],
        }
    }
    fn ready_frame(
        store: &mut Store,
        id: SurfaceId,
        owner: u64,
        _buffer: BufferId,
        sequence: u64,
    ) -> Frame {
        let config = store.broker.configuration(id).unwrap();
        if sequence == 1 {
            store
                .broker
                .acknowledge_object(id, owner, config.generation)
                .unwrap();
        }
        Frame {
            surface_generation: id.generation(),
            configure_generation: config.generation,
            frame_generation: sequence,
            buffer_handle: 1,
            logical_width: 2,
            logical_height: 2,
            pixel_width: 2,
            pixel_height: 2,
            scale_16_16: 65536,
            stride_bytes: 8,
            buffer_bytes: 16,
            private_content: false,
            format: next_abi::surface::PixelFormat::Rgbx,
            damage_rectangles: [next_abi::surface::Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            }; next_abi::surface::MAX_DAMAGE],
            damage_count: 1,
        }
    }
    #[test]
    fn actual_try_guard_backpressure_preserves_ticket_for_retry_then_rejects_closed_generation() {
        let guard = Guard::acquire().unwrap();
        let (id, buffer, ticket, frame) = {
            let store = unsafe { &mut *STORE.0.get() };
            let id = store.create(5011, origin(5011), 2, 2).unwrap();
            let buffer = store.import(5011, &[5; 16]).unwrap();
            let frame = ready_frame(store, id, 5011, buffer, 1);
            store
                .broker
                .publish_objects(id, 5011, frame, buffer)
                .unwrap();
            let (_, _, _, ticket) = store.peek(id.slot(), &mut [0; 16]).unwrap().unwrap();
            (id, buffer, ticket, frame)
        };
        // Exercise the actual public try-lock boundary, without a mock refusal
        // or a separate copy of the queue/consumer implementation.
        assert_eq!(consume(id, ticket), Err(Status::WouldBlock));
        assert_eq!(consume(id, ticket), Err(Status::WouldBlock));
        assert_eq!(publication_is_current(id, ticket), Err(Status::WouldBlock));
        drop(guard);
        let mut pixels = [0; 16];
        let (_, metadata, _, preserved) = peek(id.slot(), &mut pixels).unwrap().unwrap();
        assert_eq!(metadata, frame);
        assert_eq!(preserved, ticket);
        assert_eq!(pixels, [5; 16]);
        assert_eq!(publication_is_current(id, ticket), Ok(true));
        consume(id, ticket).unwrap();
        assert_eq!(publication_is_current(id, ticket), Ok(true));
        assert!(peek(id.slot(), &mut pixels).unwrap().is_none());
        let mut next = frame;
        next.frame_generation = 2;
        publish(id, 5011, next, buffer).unwrap();
        close_surface(id).unwrap();
        close_buffer(buffer).unwrap();
        let guard = Guard::acquire().unwrap();
        let (replacement, background, buffer, replacement_ticket) = {
            let store = unsafe { &mut *STORE.0.get() };
            let replacement = store.create(5012, origin(5012), 2, 2).unwrap();
            let buffer = store.import(5012, &[9; 16]).unwrap();
            let frame = ready_frame(store, replacement, 5012, buffer, 1);
            store
                .broker
                .publish_objects(replacement, 5012, frame, buffer)
                .unwrap();
            let (_, _, _, ticket) = store
                .peek(replacement.slot(), &mut pixels)
                .unwrap()
                .unwrap();
            store.input_active[replacement.slot()] = true;
            let background = store.create(5013, origin(5013), 2, 2).unwrap();
            (replacement, background, buffer, ticket)
        };
        drop(guard);
        assert_eq!(replacement.slot(), id.slot());
        assert_ne!(replacement, id);
        assert_eq!(consume(id, ticket), Err(Status::InvalidHandle));
        assert_eq!(consume(replacement, ticket), Err(Status::InvalidHandle));
        assert_eq!(
            peek(replacement.slot(), &mut pixels).unwrap().unwrap().3,
            replacement_ticket
        );
        assert_eq!(pixels, [9; 16]);
        consume(replacement, replacement_ticket).unwrap();
        focus_gesture(replacement, 100).unwrap();
        assert_eq!(input_is_focused(replacement), Ok(true));
        assert_eq!(input_is_focused(background), Ok(false));
        assert_eq!(input_is_focused(id), Err(Status::InvalidHandle));
        let (event, input_ticket) = input_peek(replacement, 5012).unwrap();
        assert_eq!(event.payload, Payload::FocusGain);
        assert_eq!(input_peek(replacement, 5013), Err(Status::AccessDenied));
        assert_eq!(
            route_key(background, Key::Character('x'), KeyAction::Press, 101),
            Ok(false)
        );
        assert!(!input_ready(background, 5013).unwrap());
        assert_eq!(
            input_peek(replacement, 5012).unwrap(),
            (event, input_ticket)
        );
        input_consume(replacement, 5012, input_ticket).unwrap();
        assert_eq!(
            route_key(id, Key::Character('x'), KeyAction::Press, 101),
            Err(Status::InvalidHandle)
        );
        assert_eq!(
            route_key(replacement, Key::Character('a'), KeyAction::Press, 101),
            Ok(true)
        );
        assert_eq!(
            input_peek(replacement, 5012).unwrap().0.payload,
            Payload::Key {
                key: Key::Character('a'),
                action: KeyAction::Press
            }
        );
        assert_eq!(
            pointer(replacement, 2, 0, 0, 102),
            Err(Status::InvalidArgument)
        );
        suspend_input(102).unwrap();
        assert_eq!(input_is_focused(replacement), Ok(false));
        assert_eq!(
            route_key(replacement, Key::Character('b'), KeyAction::Press, 103),
            Ok(false)
        );
        close_surface(background).unwrap();
        close_surface(replacement).unwrap();
        close_buffer(buffer).unwrap();
        assert!(origins().unwrap().iter().all(Option::is_none));

        // The public resize boundary shares the same actual guard and storage;
        // keeping this in one test avoids competing global fixture owners.
        let held = Guard::acquire().unwrap();
        let (resize_id, old_buffer, old_ticket, input_ticket, prior) = {
            let store = unsafe { &mut *STORE.0.get() };
            let id = store.create(5014, origin(5014), 2, 2).unwrap();
            let buffer = store.import(5014, &[7; 16]).unwrap();
            let frame = ready_frame(store, id, 5014, buffer, 1);
            store.input_active[id.slot()] = true;
            store.closing[id.slot()].negotiate(1);
            store.focus(id, 200).unwrap();
            let (_, input_ticket) = store.input[id.slot()].peek().unwrap();
            let ticket = store
                .broker
                .publish_objects(id, 5014, frame, buffer)
                .unwrap();
            (
                id,
                buffer,
                ticket,
                input_ticket,
                store.broker.configuration(id).unwrap(),
            )
        };
        let geometry = Geometry {
            logical_width: 1,
            logical_height: 1,
            pixel_width: 1,
            pixel_height: 1,
            scale_16_16: 65536,
        };
        assert_eq!(
            reconfigure_trusted(resize_id, prior.generation, geometry, 201),
            Err(Status::WouldBlock)
        );
        assert_eq!(
            reconfigure_trusted(resize_id, prior.generation, geometry, 201),
            Err(Status::WouldBlock)
        );
        {
            let store = unsafe { &*STORE.0.get() };
            assert_eq!(store.broker.configuration(resize_id).unwrap(), prior);
            assert_eq!(store.focused, Some(resize_id));
            assert_eq!(
                store.broker.publication_is_current(resize_id, old_ticket),
                Ok(true)
            );
            assert_eq!(
                store.closing[resize_id.slot()].snapshot.phase,
                ClosePhase::Idle
            );
        }
        drop(held);
        let current = reconfigure_trusted(resize_id, prior.generation, geometry, 201).unwrap();
        assert!(current.configure.generation > prior.generation);
        assert_eq!(publication_is_current(resize_id, old_ticket), Ok(false));
        assert_eq!(consume(resize_id, old_ticket), Err(Status::InvalidHandle));
        assert_eq!(
            input_consume(resize_id, 5014, input_ticket),
            Err(Status::InvalidArgument)
        );
        assert_eq!(input_is_focused(resize_id), Ok(false));
        assert_eq!(focus_gesture(resize_id, 202), Err(Status::WouldBlock));
        assert_eq!(
            close_snapshot(resize_id, 202).unwrap().phase,
            ClosePhase::Unsupported
        );
        assert_eq!(
            reconfigure_trusted(resize_id, prior.generation, geometry, 202),
            Err(Status::InvalidArgument)
        );
        assert_eq!(configure(resize_id, 5014).unwrap(), current);
        for payload in [
            Payload::Reset {
                reason: next_abi::client_input::RESET_RECONFIGURE,
            },
            Payload::FocusLoss,
        ] {
            let (event, ticket) = input_peek(resize_id, 5014).unwrap();
            assert_eq!(event.configure_generation, current.configure.generation);
            assert_eq!(event.timestamp_ns, 201);
            assert_eq!(event.payload, payload);
            input_consume(resize_id, 5014, ticket).unwrap();
        }
        acknowledge_features(
            resize_id,
            5014,
            resize_id.generation(),
            current.configure.generation,
            1,
        )
        .unwrap();
        assert_eq!(input_is_focused(resize_id), Ok(false));
        let new_buffer = import(5014, &[9; 4]).unwrap();
        let next = {
            let held = Guard::acquire().unwrap();
            let store = unsafe { &mut *STORE.0.get() };
            let mut frame = ready_frame(store, resize_id, 5014, new_buffer, 2);
            frame.logical_width = 1;
            frame.logical_height = 1;
            frame.pixel_width = 1;
            frame.pixel_height = 1;
            frame.stride_bytes = 4;
            frame.buffer_bytes = 4;
            frame.damage_rectangles[0] = next_abi::surface::Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            };
            drop(held);
            frame
        };
        publish(resize_id, 5014, next, new_buffer).unwrap();
        let (_, metadata, _, ticket) = peek(resize_id.slot(), &mut [0; 4]).unwrap().unwrap();
        assert_eq!(metadata.configure_generation, current.configure.generation);
        assert_eq!(metadata.frame_generation, 2);
        assert_eq!(consume(resize_id, old_ticket), Err(Status::InvalidHandle));
        assert_eq!(publication_is_current(resize_id, ticket), Ok(true));
        consume(resize_id, ticket).unwrap();
        close_buffer(old_buffer).unwrap();
        close_buffer(new_buffer).unwrap();
        close_surface(resize_id).unwrap();
        assert_eq!(
            reconfigure_trusted(resize_id, current.configure.generation, geometry, 203),
            Err(Status::InvalidHandle)
        );
        assert!(origins().unwrap().iter().all(Option::is_none));
    }
    #[test]
    fn trusted_header_focus_and_minimize_preserve_surface_without_pointer_or_publish_focus() {
        let mut store = Store::new();
        let id = store.create(74, origin(74), 2, 2).unwrap();
        let buffer = store.import(74, &[8; 16]).unwrap();
        let frame = ready_frame(&mut store, id, 74, buffer, 1);
        store.input_active[id.slot()] = true;
        store.closing[id.slot()].negotiate(1);
        let ticket = store.broker.publish_objects(id, 74, frame, buffer).unwrap();
        store.broker.retain(Object::Surface(id)).unwrap();
        store.focus(id, 10).unwrap();
        let (gain, input_ticket) = store.input[id.slot()].peek().unwrap();
        assert_eq!(gain.payload, Payload::FocusGain);
        store.input[id.slot()].consume(input_ticket).unwrap();
        assert!(
            !store.input[id.slot()].ready(),
            "header focus must never invent a content pointer"
        );
        store.focus(id, 11).unwrap();
        assert!(!store.input[id.slot()].ready());
        let configuration = store.broker.configuration(id).unwrap();
        let refs = store.broker.references(Object::Surface(id)).unwrap();
        assert_eq!(store.suspend(9), Err(Status::InvalidArgument));
        assert_eq!(store.focused, Some(id));
        assert!(!store.input[id.slot()].ready());
        store.suspend(12).unwrap();
        assert_eq!(store.focused, None);
        assert_eq!(store.broker.references(Object::Surface(id)).unwrap(), refs);
        assert_eq!(store.broker.configuration(id).unwrap(), configuration);
        assert_eq!(store.broker.publication_is_current(id, ticket), Ok(true));
        let mut output = [0; 16];
        assert_eq!(
            store.peek(id.slot(), &mut output).unwrap().unwrap().3,
            ticket
        );
        assert_eq!(output, [8; 16]);
        for expected in [
            Payload::Reset {
                reason: RESET_FOCUS_LOSS,
            },
            Payload::FocusLoss,
        ] {
            let (event, ticket) = store.input[id.slot()].peek().unwrap();
            assert_eq!(event.payload, expected);
            store.input[id.slot()].consume(ticket).unwrap();
        }
        assert!(!store.input[id.slot()].ready());
        store.consume(id, ticket).unwrap();
        let mut newer = frame;
        newer.frame_generation = 2;
        store.broker.publish_objects(id, 74, newer, buffer).unwrap();
        assert_eq!(
            store.focused, None,
            "hidden/background publication cannot restore gesture authority"
        );
        assert!(!store.input[id.slot()].ready());
        store.close(Object::Surface(id)).unwrap();
        assert_eq!(store.origin(id).unwrap(), origin(74));
        store.focus(id, 13).unwrap();
        let (event, _) = store.input[id.slot()].peek().unwrap();
        assert_eq!(event.payload, Payload::FocusGain);
        store.close(Object::Surface(id)).unwrap();
        assert_eq!(store.focused, None);
        assert_eq!(store.origin(id), Err(Status::InvalidHandle));
    }
    #[test]
    fn prepared_resize_failures_preserve_publication_input_focus_and_close() {
        let mut store = Store::new();
        let id = store.create(72, origin(72), 2, 2).unwrap();
        let buffer = store.import(72, &[6; 16]).unwrap();
        let frame = ready_frame(&mut store, id, 72, buffer, 1);
        let ticket = store.broker.publish_objects(id, 72, frame, buffer).unwrap();
        store.input_active[id.slot()] = true;
        store.closing[id.slot()].negotiate(1);
        store.focus(id, 10).unwrap();
        store.request_close(id, 11).unwrap();
        let configuration = store.broker.configuration(id).unwrap();
        let close = store.closing[id.slot()].snapshot;
        let head = store.input[id.slot()].peek_record().unwrap();
        let sequence = store.input[id.slot()].last_sequence();
        let geometry = Geometry {
            logical_width: 2,
            logical_height: 2,
            pixel_width: 2,
            pixel_height: 2,
            scale_16_16: 65536,
        };
        for (expected, geometry, time, error) in [
            (
                configuration.generation + 1,
                geometry,
                12,
                Status::InvalidArgument,
            ),
            (
                configuration.generation,
                geometry,
                9,
                Status::InvalidArgument,
            ),
            (
                configuration.generation,
                Geometry {
                    pixel_width: 0,
                    ..geometry
                },
                12,
                Status::InvalidArgument,
            ),
            (
                configuration.generation,
                Geometry {
                    logical_width: 129,
                    logical_height: 129,
                    pixel_width: 129,
                    pixel_height: 129,
                    ..geometry
                },
                12,
                Status::InvalidArgument,
            ),
        ] {
            assert_eq!(
                store.reconfigure_trusted(id, expected, geometry, time),
                Err(error)
            );
            assert_eq!(store.broker.configuration(id).unwrap(), configuration);
            assert_eq!(store.input[id.slot()].peek_record().unwrap(), head);
            assert_eq!(store.input[id.slot()].last_sequence(), sequence);
            assert_eq!(store.closing[id.slot()].snapshot, close);
            assert_eq!(store.focused, Some(id));
            assert!(store.input_active[id.slot()]);
            assert_eq!(store.broker.publication_is_current(id, ticket), Ok(true));
        }
        // Two notifications need two sequence numbers. Preparing the first
        // locally must not commit it when the second would exhaust identity.
        store.input[id.slot()]
            .enqueue(Event {
                surface_generation: id.generation(),
                configure_generation: configuration.generation,
                sequence: u64::MAX - 1,
                timestamp_ns: 11,
                payload: Payload::Pointer {
                    x: 1,
                    y: 1,
                    buttons: 7,
                },
            })
            .unwrap();
        assert_eq!(
            store.reconfigure_trusted(id, configuration.generation, geometry, 12),
            Err(Status::NoMemory)
        );
        assert_eq!(store.input[id.slot()].last_sequence(), u64::MAX - 1);
        assert_eq!(store.input[id.slot()].peek_record().unwrap(), head);
        assert_eq!(store.closing[id.slot()].snapshot, close);
        assert_eq!(store.broker.configuration(id).unwrap(), configuration);
        assert_eq!(store.focused, Some(id));
        assert_eq!(store.broker.publication_is_current(id, ticket), Ok(true));
    }
    #[test]
    fn resize_reset_notifies_unfocused_owner_and_never_restarts_frame_sequence() {
        let mut store = Store::new();
        let id = store.create(73, origin(73), 2, 2).unwrap();
        let buffer = store.import(73, &[7; 16]).unwrap();
        let mut frame = ready_frame(&mut store, id, 73, buffer, 1);
        store.broker.publish_objects(id, 73, frame, buffer).unwrap();
        let old = store.broker.configuration(id).unwrap();
        let new = store
            .reconfigure_trusted(id, old.generation, old.geometry, 20)
            .unwrap();
        let (reset, ticket) = store.input[id.slot()].peek().unwrap();
        assert_eq!(reset.configure_generation, new.configure.generation);
        assert_eq!(reset.timestamp_ns, 20);
        assert_eq!(
            reset.payload,
            Payload::Reset {
                reason: next_abi::client_input::RESET_RECONFIGURE
            }
        );
        store.input[id.slot()].consume(ticket).unwrap();
        assert!(!store.input[id.slot()].ready());
        store
            .acknowledge_features(id, 73, id.generation(), new.configure.generation, 0)
            .unwrap();
        assert_eq!(store.focused, None);
        frame.configure_generation = new.configure.generation;
        assert_eq!(
            store.broker.publish_objects(id, 73, frame, buffer),
            Err(Error::State(prism::surface_state::Error::StaleFrame))
        );
        frame.frame_generation = 2;
        store.broker.publish_objects(id, 73, frame, buffer).unwrap();
    }
    #[test]
    fn adapter_reconfigure_cancels_old_publication_and_preserves_new_ticket() {
        let mut store = Store::new();
        let id = store.create(71, origin(71), 2, 2).unwrap();
        let buffer = store.import(71, &[4; 16]).unwrap();
        let mut frame = ready_frame(&mut store, id, 71, buffer, 1);
        let old = store.broker.publish_objects(id, 71, frame, buffer).unwrap();
        let (_, _, _, copied) = store.peek(id.slot(), &mut [0; 16]).unwrap().unwrap();
        assert_eq!(old, copied);
        store
            .input_configuration(
                id,
                Geometry {
                    logical_width: 2,
                    logical_height: 2,
                    pixel_width: 2,
                    pixel_height: 2,
                    scale_16_16: 65536,
                },
                10,
            )
            .unwrap();
        assert_eq!(store.broker.publication_is_current(id, copied), Ok(false));
        assert_eq!(store.consume(id, copied), Err(Status::InvalidHandle));
        assert!(store.peek(id.slot(), &mut [0; 16]).unwrap().is_none());
        let config = store.broker.configuration(id).unwrap();
        store
            .acknowledge_features(id, 71, id.generation(), config.generation, 0)
            .unwrap();
        frame.configure_generation = config.generation;
        frame.frame_generation = 2;
        let new = store.broker.publish_objects(id, 71, frame, buffer).unwrap();
        assert_eq!(store.consume(id, copied), Err(Status::InvalidHandle));
        assert_eq!(store.peek(id.slot(), &mut [0; 16]).unwrap().unwrap().3, new);
        store.consume(id, new).unwrap();
        assert_eq!(store.broker.publication_is_current(id, new), Ok(true));
    }
    #[test]
    fn close_between_peek_and_consume_cannot_consume_recreated_surface_publication() {
        let mut store = std::boxed::Box::new(Store::new());
        let id = store.create(7, origin(7), 2, 2).unwrap();
        let buffer = store.import(7, &[1; 16]).unwrap();
        let frame = ready_frame(&mut store, id, 7, buffer, 1);
        store.broker.publish_objects(id, 7, frame, buffer).unwrap();
        let mut staging = [0; BUFFER_BYTES];
        let (old_id, _, old_origin, old_ticket) =
            store.peek(id.slot(), &mut staging).unwrap().unwrap();
        assert_eq!(old_origin, origin(7));
        assert_eq!(&staging[..16], &[1; 16]);
        // Process exit drops its final surface/buffer refs between main's
        // independent staging copy and the publication acknowledgment.
        store.close(Object::Surface(id)).unwrap();
        store.close(Object::Buffer(buffer)).unwrap();
        assert!(store.origins().iter().all(Option::is_none));
        let replacement = store.create(8, origin(8), 2, 2).unwrap();
        assert_eq!(old_id.slot(), replacement.slot());
        assert_ne!(old_id, replacement);
        let buffer = store.import(8, &[9; 16]).unwrap();
        let frame = ready_frame(&mut store, replacement, 8, buffer, 1);
        store
            .broker
            .publish_objects(replacement, 8, frame, buffer)
            .unwrap();
        assert_eq!(
            store.consume(old_id, old_ticket),
            Err(Status::InvalidHandle)
        );
        assert_eq!(
            store.consume(replacement, old_ticket),
            Err(Status::InvalidHandle)
        );
        assert_eq!(store.origin(old_id), Err(Status::InvalidHandle));
        let (new_id, _, new_origin, new_ticket) = store
            .peek(replacement.slot(), &mut staging)
            .unwrap()
            .unwrap();
        assert_eq!(new_id, replacement);
        assert_eq!(new_origin, origin(8));
        assert_eq!(&staging[..16], &[9; 16]);
        store.consume(replacement, new_ticket).unwrap();
        assert!(store
            .peek(replacement.slot(), &mut staging)
            .unwrap()
            .is_none());
        store.close(Object::Surface(replacement)).unwrap();
        assert!(store.origins().iter().all(Option::is_none));
    }
    #[test]
    fn duplicate_last_close_lifecycle_snapshot_and_backpressure_track_exact_objects() {
        let mut store = std::boxed::Box::new(Store::new());
        let first = store.create(7, origin(7), 2, 2).unwrap();
        let second = store.create(8, origin(8), 2, 2).unwrap();
        let buffer = store.import(7, &[3; 16]).unwrap();
        let frame = ready_frame(&mut store, first, 7, buffer, 1);
        store
            .broker
            .publish_objects(first, 7, frame, buffer)
            .unwrap();
        store.broker.retain(Object::Surface(first)).unwrap();
        store.close(Object::Surface(first)).unwrap();
        assert!(store
            .origins()
            .iter()
            .flatten()
            .any(|(id, origin)| *id == first && origin.owner == 7));
        let mut next = frame;
        next.frame_generation = 2;
        assert_eq!(
            store.broker.publish_objects(first, 7, next, buffer),
            Err(Error::WouldBlock)
        );
        let mut staging = [0; BUFFER_BYTES];
        let (_, _, _, ticket) = store.peek(first.slot(), &mut staging).unwrap().unwrap();
        // A rejected/short staging copy does not release backpressure.
        assert_eq!(
            store.peek(first.slot(), &mut [0; 15]),
            Err(Status::InvalidArgument)
        );
        assert_eq!(
            store.broker.publish_objects(first, 7, next, buffer),
            Err(Error::WouldBlock)
        );
        store.consume(first, ticket).unwrap();
        store
            .broker
            .publish_objects(first, 7, next, buffer)
            .unwrap();
        assert_eq!(store.consume(first, ticket), Err(Status::InvalidHandle));
        store.close(Object::Surface(first)).unwrap();
        assert!(store.peek(first.slot(), &mut staging).unwrap().is_none());
        assert_eq!(
            store
                .origins()
                .iter()
                .flatten()
                .map(|(id, _)| *id)
                .collect::<std::vec::Vec<_>>(),
            std::vec![second]
        );
        let recreated = store.create(9, origin(9), 2, 2).unwrap();
        let origins = store.origins();
        assert!(origins
            .iter()
            .flatten()
            .any(|(id, origin)| *id == recreated && origin.owner == 9));
        assert!(origins
            .iter()
            .flatten()
            .any(|(id, origin)| *id == second && origin.owner == 8));
        assert_eq!(store.origin(first), Err(Status::InvalidHandle));
        // Metadata packing order is not the broker/DMA/UI slot identity. A
        // retired slot or changed metadata layout must still join by full ID.
        store.surfaces.swap(0, 1);
        let second_buffer = store.import(8, &[8; 16]).unwrap();
        let second_frame = ready_frame(&mut store, second, 8, second_buffer, 1);
        store
            .broker
            .publish_objects(second, 8, second_frame, second_buffer)
            .unwrap();
        let (id, _, origin, _) = store.peek(second.slot(), &mut staging).unwrap().unwrap();
        assert_eq!(id, second);
        assert_eq!(origin.owner, 8);
        assert_eq!(&staging[..16], &[8; 16]);
    }
    #[test]
    fn actual_per_owner_quotas_duplicate_lifetime_and_teardown_are_enforced() {
        let mut store = std::boxed::Box::new(Store::new());
        let first = store.create(7, origin(7), 2, 2).unwrap();
        assert_eq!(store.create(7, origin(7), 2, 2), Err(Status::NoMemory));
        let second = store.create(8, origin(8), 2, 2).unwrap();
        assert_eq!(store.owner(first, 8), Err(Status::AccessDenied));
        store.broker.retain(Object::Surface(first)).unwrap();
        store.close(Object::Surface(first)).unwrap();
        assert!(store.origin(first).is_ok());
        store.close(Object::Surface(first)).unwrap();
        assert_eq!(store.origin(first), Err(Status::InvalidHandle));
        let replacement = store.create(7, origin(7), 2, 2).unwrap();
        assert_ne!(first, replacement);
        let a = store.import(7, &[1; 16]).unwrap();
        let b = store.import(7, &[2; 16]).unwrap();
        assert_eq!(store.import(7, &[3; 16]), Err(Status::NoMemory));
        store.close(Object::Buffer(a)).unwrap();
        store.close(Object::Buffer(b)).unwrap();
        store.close(Object::Surface(replacement)).unwrap();
        store.close(Object::Surface(second)).unwrap();
        assert!(store.surfaces.iter().all(Option::is_none));
        assert!(store.buffers.iter().all(Option::is_none));
    }
    #[test]
    fn input_owner_focus_overflow_and_last_reference_reuse() {
        let mut store = std::boxed::Box::new(Store::new());
        let id = store.create(11, origin(11), 8, 8).unwrap();
        assert_eq!(store.focus(id, 1), Err(Status::WouldBlock));
        let config = store.broker.configuration(id).unwrap();
        store
            .broker
            .acknowledge_object(id, 11, config.generation)
            .unwrap();
        store.input_active[id.slot()] = true;
        assert_eq!(store.owner(id, 12), Err(Status::AccessDenied));
        store.focus(id, 2).unwrap();
        let (gain, ticket) = store.input[id.slot()].peek().unwrap();
        assert_eq!(gain.payload, Payload::FocusGain);
        let sequence = store.input[id.slot()].last_sequence();
        assert_eq!(
            store.emit(
                id,
                Payload::Pointer {
                    x: 1,
                    y: 1,
                    buttons: 0
                },
                1
            ),
            Err(Status::InvalidArgument)
        );
        assert_eq!(store.input[id.slot()].last_sequence(), sequence);
        assert_eq!(store.input[id.slot()].peek().unwrap(), (gain, ticket));
        assert_eq!(gain.timestamp_ns, 2);
        assert!(store.input[id.slot()].ready());
        assert_eq!(store.input[id.slot()].peek().unwrap().1, ticket);
        store.input[id.slot()].consume(ticket).unwrap();
        for timestamp in 3..19 {
            store
                .emit(
                    id,
                    Payload::Key {
                        key: Key::Character('a'),
                        action: KeyAction::Press,
                    },
                    timestamp,
                )
                .unwrap();
        }
        assert_eq!(
            store.emit(
                id,
                Payload::Pointer {
                    x: 1,
                    y: 1,
                    buttons: 1
                },
                19
            ),
            Err(Status::WouldBlock)
        );
        assert_eq!(store.focused, None);
        let (reset, reset_ticket) = store.input[id.slot()].peek().unwrap();
        assert_eq!(
            reset.payload,
            Payload::Reset {
                reason: next_abi::client_input::RESET_OVERFLOW
            }
        );
        store.input[id.slot()].consume(reset_ticket).unwrap();
        assert_eq!(
            store.input[id.slot()].peek().unwrap().0.payload,
            Payload::FocusLoss
        );
        store.broker.retain(Object::Surface(id)).unwrap();
        store.close(Object::Surface(id)).unwrap();
        assert!(store.input[id.slot()].ready());
        store.close(Object::Surface(id)).unwrap();
        assert!(!store.input[id.slot()].ready());
        let replacement = store.create(11, origin(11), 8, 8).unwrap();
        assert_eq!(replacement.slot(), id.slot());
        assert_ne!(replacement.generation(), id.generation());
        assert_eq!(store.origin(id), Err(Status::InvalidHandle));
        let config = store.broker.configuration(replacement).unwrap();
        store
            .broker
            .acknowledge_object(replacement, 11, config.generation)
            .unwrap();
        store.input_active[replacement.slot()] = true;
        store.focus(replacement, 20).unwrap();
        assert_eq!(
            store.input[replacement.slot()].consume(ticket),
            Err(InputError::Stale)
        );
        assert_eq!(
            store.input[replacement.slot()].peek().unwrap().0.payload,
            Payload::FocusGain
        );
    }
    #[test]
    fn input_reconfigure_clears_old_coordinates_and_modal_suspends() {
        let mut store = std::boxed::Box::new(Store::new());
        let id = store.create(12, origin(12), 8, 8).unwrap();
        let config = store.broker.configuration(id).unwrap();
        store
            .broker
            .acknowledge_object(id, 12, config.generation)
            .unwrap();
        store.input_active[id.slot()] = true;
        store.focus(id, 10).unwrap();
        let (_, old_ticket) = store.input[id.slot()].peek().unwrap();
        store
            .emit(
                id,
                Payload::Pointer {
                    x: 7,
                    y: 7,
                    buttons: 1,
                },
                11,
            )
            .unwrap();
        store
            .input_configuration(
                id,
                Geometry {
                    logical_width: 4,
                    logical_height: 4,
                    pixel_width: 4,
                    pixel_height: 4,
                    scale_16_16: 65536,
                },
                12,
            )
            .unwrap();
        assert_eq!(store.focused, None);
        assert_eq!(store.focus(id, 13), Err(Status::WouldBlock));
        assert_eq!(
            store.input[id.slot()].consume(old_ticket),
            Err(InputError::Stale)
        );
        let (reset, ticket) = store.input[id.slot()].peek().unwrap();
        assert_eq!(reset.configure_generation, config.generation + 1);
        assert_eq!(
            reset.payload,
            Payload::Reset {
                reason: next_abi::client_input::RESET_RECONFIGURE
            }
        );
        store.input[id.slot()].consume(ticket).unwrap();
        let (_, ticket) = store.input[id.slot()].peek().unwrap();
        store.input[id.slot()].consume(ticket).unwrap();
        let config = store.broker.configuration(id).unwrap();
        store
            .broker
            .acknowledge_object(id, 12, config.generation)
            .unwrap();
        store.input_active[id.slot()] = true;
        store.focus(id, 14).unwrap();
        store.suspend(15).unwrap();
        assert_eq!(store.focused, None);
        assert!(store.input[id.slot()].ready());
    }
    fn close_identity(request: CloseRequest) -> next_abi::client_close::Request {
        next_abi::client_close::Request {
            surface_generation: request.surface_generation,
            configure_generation: request.configure_generation,
            ticket: request.ticket,
            sequence: request.sequence,
        }
    }
    fn negotiated(store: &mut Store, owner: u64) -> SurfaceId {
        let id = store.create(owner, origin(owner), 2, 2).unwrap();
        let config = store.broker.configuration(id).unwrap();
        store
            .acknowledge_features(
                id,
                owner,
                id.generation(),
                config.generation,
                next_abi::client_close::FEATURE_CLOSE_REQUEST,
            )
            .unwrap();
        id
    }
    #[test]
    fn negotiated_close_preserves_legacy_head_and_advisory_replies_never_release_caps() {
        let mut store = std::boxed::Box::new(Store::new());
        let id = store.create(601, origin(601), 2, 2).unwrap();
        let config = store.broker.configuration(id).unwrap();
        assert_eq!(
            store.acknowledge_features(id, 601, id.generation(), config.generation, 2),
            Err(Status::InvalidArgument)
        );
        assert_eq!(store.request_close(id, 10), Err(Status::NotSupported));
        store
            .acknowledge_features(id, 601, id.generation(), config.generation, 1)
            .unwrap();
        store.focus(id, 10).unwrap();
        let (_, ticket) = store.input[id.slot()].peek().unwrap();
        store.input[id.slot()].consume(ticket).unwrap();
        let request = store.request_close(id, 11).unwrap().request.unwrap();
        assert_eq!(store.request_close(id, 12), Err(Status::WouldBlock));
        assert_eq!(store.input[id.slot()].peek(), Err(InputError::Unsupported));
        let (record, ticket) = store.input[id.slot()].peek_record().unwrap();
        assert_eq!(record.encode(0), Err(InputError::Unsupported));
        let wire = record.encode(2).unwrap();
        let decoded = next_abi::client_close::Event::decode(&wire).unwrap();
        assert_eq!(decoded.close_request().unwrap(), close_identity(request));
        assert_eq!(
            store.input[id.slot()].peek_record().unwrap(),
            (record, ticket)
        );
        // Repeated mode negotiation is rejected by already-acknowledged state.
        assert_eq!(
            store.acknowledge_features(id, 601, id.generation(), config.generation, 1),
            Err(Status::InvalidArgument)
        );
        assert_eq!(
            store.acknowledge_features(id, 601, id.generation(), config.generation, 0),
            Err(Status::InvalidArgument)
        );
        assert_eq!(store.closing[id.slot()].snapshot.request, Some(request));
        assert_eq!(
            store.input[id.slot()].peek_record().unwrap(),
            (record, ticket)
        );
        let identity = close_identity(request);
        assert_eq!(
            store.close_reply(id, 602, identity, 1, 12),
            Err(Status::AccessDenied)
        );
        assert_eq!(
            store.close_reply(id, 601, identity, 0, 12),
            Err(Status::InvalidArgument)
        );
        assert_eq!(
            store.close_reply(
                id,
                601,
                next_abi::client_close::Request {
                    ticket: request.ticket + 1,
                    ..identity
                },
                1,
                12
            ),
            Err(Status::InvalidHandle)
        );
        assert_eq!(store.closing[id.slot()].snapshot.phase, ClosePhase::Pending);
        store.input[id.slot()].consume(ticket).unwrap();
        store.close_reply(id, 601, identity, 1, 12).unwrap();
        assert_eq!(store.closing[id.slot()].snapshot.phase, ClosePhase::Refused);
        assert_eq!(store.focused, Some(id));
        assert_eq!(store.broker.references(Object::Surface(id)).unwrap(), 1);
        assert_eq!(
            store.close_reply(id, 601, identity, 1, 13),
            Err(Status::InvalidHandle)
        );
        let next = store.request_close(id, 14).unwrap().request.unwrap();
        assert_ne!(next.ticket, request.ticket);
        store
            .close_reply(id, 601, close_identity(next), 2, 15)
            .unwrap();
        assert_eq!(store.closing[id.slot()].snapshot.phase, ClosePhase::Closing);
        assert!(store.origin(id).is_ok());
        assert_eq!(store.focused, Some(id));
        assert_eq!(
            store.close_reply(id, 601, close_identity(next), 2, 16),
            Err(Status::InvalidHandle)
        );
    }
    #[test]
    fn close_overflow_reconfigure_and_last_close_invalidate_exact_request() {
        let mut store = std::boxed::Box::new(Store::new());
        let id = negotiated(&mut store, 611);
        store.focus(id, 1).unwrap();
        let (_, ticket) = store.input[id.slot()].peek().unwrap();
        store.input[id.slot()].consume(ticket).unwrap();
        let request = store.request_close(id, 2).unwrap().request.unwrap();
        for now in 3..18 {
            store
                .emit(
                    id,
                    Payload::Key {
                        key: Key::Character('x'),
                        action: KeyAction::Press,
                    },
                    now,
                )
                .unwrap();
        }
        assert_eq!(
            store.emit(
                id,
                Payload::Key {
                    key: Key::Enter,
                    action: KeyAction::Press
                },
                18
            ),
            Err(Status::WouldBlock)
        );
        assert_eq!(store.focused, None);
        assert_eq!(store.closing[id.slot()].snapshot.phase, ClosePhase::Idle);
        assert_eq!(
            store.close_reply(id, 611, close_identity(request), 2, 19),
            Err(Status::InvalidHandle)
        );
        assert!(matches!(
            store.input[id.slot()].peek().unwrap().0.payload,
            Payload::Reset { .. }
        ));
        while store.input[id.slot()].ready() {
            let (_, ticket) = store.input[id.slot()].peek().unwrap();
            store.input[id.slot()].consume(ticket).unwrap();
        }
        let request = store.request_close(id, 20).unwrap().request.unwrap();
        let (_, ticket) = store.input[id.slot()].peek_record().unwrap();
        store
            .input_configuration(
                id,
                Geometry {
                    logical_width: 1,
                    logical_height: 1,
                    pixel_width: 1,
                    pixel_height: 1,
                    scale_16_16: 65536,
                },
                21,
            )
            .unwrap();
        assert_eq!(
            store.closing[id.slot()].snapshot.phase,
            ClosePhase::Unsupported
        );
        assert_eq!(
            store.input[id.slot()].consume(ticket),
            Err(InputError::Stale)
        );
        assert_eq!(
            store.close_reply(id, 611, close_identity(request), 1, 22),
            Err(Status::InvalidHandle)
        );
        let config = store.broker.configuration(id).unwrap();
        store
            .acknowledge_features(id, 611, id.generation(), config.generation, 1)
            .unwrap();
        let request = store.request_close(id, 23).unwrap().request.unwrap();
        store.broker.retain(Object::Surface(id)).unwrap();
        store.close(Object::Surface(id)).unwrap();
        assert_eq!(store.closing[id.slot()].snapshot.request, Some(request));
        store.close(Object::Surface(id)).unwrap();
        let replacement = negotiated(&mut store, 611);
        assert_eq!(replacement.slot(), id.slot());
        assert_ne!(replacement.generation(), id.generation());
        assert_eq!(
            store.close_reply(id, 611, close_identity(request), 2, 24),
            Err(Status::InvalidHandle)
        );
        assert_eq!(
            store.close_reply(replacement, 611, close_identity(request), 2, 24),
            Err(Status::InvalidHandle)
        );
        assert!(!store.input[replacement.slot()].ready());
    }
    #[test]
    fn close_expiry_counter_and_deadline_failures_never_mutate_surface_lifetime() {
        let mut store = std::boxed::Box::new(Store::new());
        let id = negotiated(&mut store, 621);
        let sequence = store.input[id.slot()].last_sequence();
        assert_eq!(
            store.request_close(id, u64::MAX),
            Err(Status::InvalidArgument)
        );
        assert_eq!(store.input[id.slot()].last_sequence(), sequence);
        assert!(!store.input[id.slot()].ready());
        let request = store.request_close(id, 10).unwrap().request.unwrap();
        store.closing[id.slot()].expire(request.deadline_ns);
        assert_eq!(
            store.closing[id.slot()].snapshot.phase,
            ClosePhase::Unresponsive
        );
        assert_eq!(
            store.close_reply(id, 621, close_identity(request), 2, request.deadline_ns + 1),
            Err(Status::TimedOut)
        );
        assert_eq!(
            store.close_reply(
                id,
                621,
                next_abi::client_close::Request {
                    sequence: request.sequence + 1,
                    ..close_identity(request)
                },
                2,
                request.deadline_ns + 1
            ),
            Err(Status::InvalidHandle)
        );
        assert_eq!(store.broker.references(Object::Surface(id)).unwrap(), 1);
        assert!(store.origin(id).is_ok());
        let (_, ticket) = store.input[id.slot()].peek_record().unwrap();
        store.input[id.slot()].consume(ticket).unwrap();
        store.close_counter = u64::MAX;
        assert_eq!(
            store.request_close(id, request.deadline_ns + 2),
            Err(Status::NoMemory)
        );
        assert!(!store.input[id.slot()].ready());
        assert_eq!(
            store.closing[id.slot()].snapshot.phase,
            ClosePhase::Unresponsive
        );
    }
    #[test]
    fn exhausted_input_sequence_cancels_pending_close_and_never_reuses_identity() {
        let mut store = std::boxed::Box::new(Store::new());
        let id = negotiated(&mut store, 631);
        store.focus(id, 1).unwrap();
        let (_, ticket) = store.input[id.slot()].peek().unwrap();
        store.input[id.slot()].consume(ticket).unwrap();
        store.input[id.slot()]
            .enqueue(Event {
                surface_generation: id.generation(),
                configure_generation: 1,
                sequence: u64::MAX - 1,
                timestamp_ns: 2,
                payload: Payload::FocusGain,
            })
            .unwrap();
        let request = store.request_close(id, 3).unwrap().request.unwrap();
        assert_eq!(request.sequence, u64::MAX);
        assert_eq!(
            store.emit(
                id,
                Payload::Key {
                    key: Key::Enter,
                    action: KeyAction::Press
                },
                4
            ),
            Err(Status::NoMemory)
        );
        assert_eq!(store.focused, None);
        assert_eq!(store.closing[id.slot()].snapshot.phase, ClosePhase::Idle);
        assert!(!store.input[id.slot()].ready());
        assert_eq!(store.input[id.slot()].last_sequence(), u64::MAX);
        assert_eq!(
            store.close_reply(id, 631, close_identity(request), 2, 5),
            Err(Status::InvalidHandle)
        );
        assert_eq!(store.request_close(id, 6), Err(Status::NoMemory));
        assert!(store.origin(id).is_ok());
    }
}
