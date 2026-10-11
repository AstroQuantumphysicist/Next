//! Trusted chooser adapter. Approval holds the selected existing inode, so a
//! later rename/unlink/replacement cannot redirect consent to a different file.
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
pub use helix_core::portal::ProcessIdentity;
use helix_core::{
    handle::Rights,
    portal::{Access, Error, Portal, RequestId, State},
};
use next_abi::Status;
const MAX_WAIT_NS: u64 = 30_000_000_000;
#[derive(Clone, Copy)]
struct Held {
    id: RequestId,
    recipient: ProcessIdentity,
    file: crate::file::FileHandle,
}
struct Broker {
    portal: Portal<4>,
    held: [Option<Held>; 4],
}
impl Broker {
    const fn new() -> Self {
        Self {
            portal: Portal::new(1),
            held: [None; 4],
        }
    }
}
struct Global(UnsafeCell<Broker>);
// Single boot CPU; references are guarded and never escape. No IRQ portal users.
unsafe impl Sync for Global {}
static BROKER: Global = Global(UnsafeCell::new(Broker::new()));
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
trait FileBackend {
    fn open(&mut self, path: &str) -> Result<crate::file::FileHandle, Status>;
    fn close(&mut self, file: crate::file::FileHandle);
}
struct KernelFiles;
impl FileBackend for KernelFiles {
    fn open(&mut self, path: &str) -> Result<crate::file::FileHandle, Status> {
        crate::file::open(path, 0)
    }
    fn close(&mut self, file: crate::file::FileHandle) {
        let _ = crate::file::close(file);
    }
}
fn status(error: Error) -> Status {
    match error {
        Error::Full => Status::NoMemory,
        Error::Deadline => Status::TimedOut,
        Error::RecipientMismatch | Error::RightsMismatch | Error::InvalidIdentity => {
            Status::AccessDenied
        }
        Error::InvalidRequest => Status::InvalidHandle,
        Error::Pending | Error::NotApproved => Status::WouldBlock,
        Error::InvalidPath | Error::ClockRegression | Error::Terminal => Status::InvalidArgument,
    }
}
fn access(value: u64) -> Result<Access, Status> {
    match value {
        1 => Ok(Access::ReadOnly),
        3 => Ok(Access::ReadWrite),
        _ => Err(Status::InvalidArgument),
    }
}
fn wire(access: Access) -> u64 {
    match access {
        Access::ReadOnly => 1,
        Access::ReadWrite => 3,
    }
}
fn rights(access: Access) -> Rights {
    match access {
        Access::ReadOnly => Rights::READ,
        Access::ReadWrite => Rights::READ.union(Rights::WRITE),
    }
}
fn now() -> u64 {
    crate::timer::nanoseconds()
}
fn deadline_valid(deadline: u64, time: u64) -> bool {
    deadline
        .checked_sub(time)
        .is_some_and(|delta| delta > 0 && delta <= MAX_WAIT_NS)
}
fn expire_with(broker: &mut Broker, files: &mut impl FileBackend, time: u64) -> Result<(), Status> {
    broker.portal.expire(time).map_err(status)?;
    for held in &mut broker.held {
        if held.is_some_and(|h| broker.portal.state(h.id, h.recipient, time) != Ok(State::Approved))
        {
            files.close(held.take().unwrap().file);
        }
    }
    Ok(())
}
fn approval_with(
    broker: &mut Broker,
    files: &mut impl FileBackend,
    identity: ProcessIdentity,
    id: RequestId,
    time: u64,
) -> Result<helix_core::portal::Approval, Status> {
    expire_with(broker, files, time)?;
    let terminal = match broker.portal.state(id, identity, time).map_err(status)? {
        State::Pending => return Err(Status::WouldBlock),
        State::Approved => return broker.portal.approved(id, identity, time).map_err(status),
        State::Denied => Status::AccessDenied,
        State::Cancelled => Status::Cancelled,
        State::Expired => Status::TimedOut,
        State::Consumed => Status::InvalidHandle,
    };
    broker.portal.release(id, identity, time).map_err(status)?;
    Err(terminal)
}
fn approve_with(
    broker: &mut Broker,
    files: &mut impl FileBackend,
    id: RequestId,
    path: &str,
    granted: Access,
    time: u64,
) -> Result<(), Status> {
    expire_with(broker, files, time)?;
    let request = broker
        .portal
        .pending(time)
        .map_err(status)?
        .find(|r| r.id == id)
        .ok_or(Status::InvalidHandle)?;
    if !path.starts_with("/native/") || path.len() > 31 {
        return Err(Status::InvalidArgument);
    }
    if request.requested == Access::ReadOnly && granted == Access::ReadWrite {
        return Err(Status::AccessDenied);
    }
    let index = broker
        .held
        .iter()
        .position(Option::is_none)
        .ok_or(Status::NoMemory)?;
    // No create/truncate: allocate the selected inode reference before approval.
    let file = files.open(path)?;
    if let Err(error) = broker
        .portal
        .approve(id, request.recipient, path, granted, time)
    {
        files.close(file);
        return Err(status(error));
    }
    broker.held[index] = Some(Held {
        id,
        recipient: request.recipient,
        file,
    });
    Ok(())
}
fn consume_with(
    broker: &mut Broker,
    files: &mut impl FileBackend,
    identity: ProcessIdentity,
    id: RequestId,
    time: u64,
) -> Result<(crate::file::FileHandle, Rights), Status> {
    let approved = approval_with(broker, files, identity, id, time)?;
    let index = broker
        .held
        .iter()
        .position(|h| h.is_some_and(|h| h.id == id && h.recipient == identity))
        .ok_or(Status::IoError)?;
    // Caller reserved EXACT attenuated rights before entering this transaction.
    let grant = broker.portal.consume(id, identity, time).map_err(status)?;
    let held = broker.held[index].take().unwrap();
    debug_assert_eq!(grant.access(), approved.access());
    if let Err(error) = broker.portal.release(id, identity, time) {
        files.close(held.file);
        return Err(status(error));
    }
    Ok((held.file, rights(grant.access())))
}
fn cancel_with(
    broker: &mut Broker,
    files: &mut impl FileBackend,
    identity: ProcessIdentity,
    id: RequestId,
    time: u64,
) -> Result<(), Status> {
    expire_with(broker, files, time)?;
    match broker.portal.state(id, identity, time).map_err(status)? {
        State::Pending | State::Approved => {
            broker.portal.cancel(id, identity, time).map_err(status)?;
            // Keep Cancelled queryable until the first poll acknowledges it.
            expire_with(broker, files, time)
        }
        _ => broker.portal.release(id, identity, time).map_err(status),
    }
}
fn died_with(broker: &mut Broker, files: &mut impl FileBackend, identity: ProcessIdentity) {
    for held in &mut broker.held {
        if held.is_some_and(|h| h.recipient == identity) {
            files.close(held.take().unwrap().file);
        }
    }
    broker.portal.process_died(identity);
}
pub fn request(identity: ProcessIdentity, requested: u64, deadline: u64) -> Result<u64, Status> {
    let requested = access(requested)?;
    let time = now();
    if !deadline_valid(deadline, time) {
        return Err(Status::InvalidArgument);
    }
    let _guard = Guard::acquire()?;
    let broker = unsafe { &mut *BROKER.0.get() };
    expire_with(broker, &mut KernelFiles, time)?;
    broker
        .portal
        .request(identity, requested, deadline, time)
        .map(RequestId::raw)
        .map_err(status)
}
/// Runtime authorizes/reserves EXACT approved rights before calling consume.
pub fn approved_rights(identity: ProcessIdentity, id: u64) -> Result<Rights, Status> {
    let _guard = Guard::acquire()?;
    let broker = unsafe { &mut *BROKER.0.get() };
    approval_with(
        broker,
        &mut KernelFiles,
        identity,
        RequestId::from_raw(id),
        now(),
    )
    .map(|choice| rights(choice.access()))
}
pub fn consume(
    identity: ProcessIdentity,
    id: u64,
) -> Result<(crate::file::FileHandle, Rights), Status> {
    let _guard = Guard::acquire()?;
    consume_with(
        unsafe { &mut *BROKER.0.get() },
        &mut KernelFiles,
        identity,
        RequestId::from_raw(id),
        now(),
    )
}
pub fn cancel(identity: ProcessIdentity, id: u64) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    cancel_with(
        unsafe { &mut *BROKER.0.get() },
        &mut KernelFiles,
        identity,
        RequestId::from_raw(id),
        now(),
    )
}
/// Trusted teardown on the single boot CPU; no portal calls occur in IRQs.
pub fn process_died(identity: ProcessIdentity) {
    if let Ok(_guard) = Guard::acquire() {
        died_with(unsafe { &mut *BROKER.0.get() }, &mut KernelFiles, identity);
    }
}
#[derive(Clone, Copy)]
pub struct DisplayRequest {
    pub id: u64,
    pub pid: u64,
    pub access: u64,
}
pub fn first_pending() -> Option<DisplayRequest> {
    let _guard = Guard::acquire().ok()?;
    let broker = unsafe { &mut *BROKER.0.get() };
    let time = now();
    expire_with(broker, &mut KernelFiles, time).ok()?;
    let request = broker.portal.pending(time).ok()?.next()?;
    Some(DisplayRequest {
        id: request.id.raw(),
        pid: request.recipient.pid(),
        access: wire(request.requested),
    })
}
/// Trusted UI only: selected existing canonical native path, never app input.
pub fn approve(id: u64, path: &str, granted: u64) -> Result<(), Status> {
    let granted = access(granted)?;
    let _guard = Guard::acquire()?;
    approve_with(
        unsafe { &mut *BROKER.0.get() },
        &mut KernelFiles,
        RequestId::from_raw(id),
        path,
        granted,
        now(),
    )
}
pub fn deny(id: u64) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    let broker = unsafe { &mut *BROKER.0.get() };
    let time = now();
    expire_with(broker, &mut KernelFiles, time)?;
    let recipient = broker
        .portal
        .pending(time)
        .map_err(status)?
        .find(|r| r.id.raw() == id)
        .ok_or(Status::InvalidHandle)?
        .recipient;
    broker
        .portal
        .deny(RequestId::from_raw(id), recipient, time)
        .map_err(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::file::{Backend, Error as FileError, Files, FILE_BYTES};
    use std::{collections::BTreeMap, string::String, vec::Vec};
    #[derive(Default)]
    struct Disk(BTreeMap<String, Vec<u8>>);
    impl Backend for Disk {
        fn read(&mut self, path: &str, out: &mut [u8; FILE_BYTES]) -> Result<usize, FileError> {
            let bytes = self.0.get(path).ok_or(FileError::NotFound)?;
            out[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        }
        fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), FileError> {
            self.0.insert(String::from(path), bytes.to_vec());
            Ok(())
        }
        fn rename(&mut self, old: &str, new: &str) -> Result<(), FileError> {
            let bytes = self.0.remove(old).ok_or(FileError::NotFound)?;
            self.0.insert(String::from(new), bytes);
            Ok(())
        }
        fn remove(&mut self, path: &str) -> Result<(), FileError> {
            self.0.remove(path).ok_or(FileError::NotFound)?;
            Ok(())
        }
    }
    struct Model {
        files: Files<8>,
        disk: Disk,
        live: usize,
        opens: usize,
        closed: usize,
        fail: bool,
    }
    impl Model {
        fn new() -> Self {
            let mut disk = Disk::default();
            disk.put("/native/a", b"selected").unwrap();
            Self {
                files: Files::new(),
                disk,
                live: 0,
                opens: 0,
                closed: 0,
                fail: false,
            }
        }
    }
    impl FileBackend for Model {
        fn open(&mut self, path: &str) -> Result<crate::file::FileHandle, Status> {
            self.opens += 1;
            if self.fail {
                return Err(Status::NoMemory);
            }
            let handle = self
                .files
                .open(&mut self.disk, path, false, false)
                .map_err(|e| match e {
                    FileError::NotFound => Status::NotFound,
                    FileError::Full => Status::NoMemory,
                    _ => Status::InvalidArgument,
                })?;
            self.live += 1;
            Ok(handle)
        }
        fn close(&mut self, file: crate::file::FileHandle) {
            self.files.close(file).unwrap();
            self.live -= 1;
            self.closed += 1;
        }
    }
    fn identity(pid: u64) -> ProcessIdentity {
        ProcessIdentity::new(pid, 1, [1; 32]).unwrap()
    }
    fn request(broker: &mut Broker, pid: u64, access: Access) -> RequestId {
        broker
            .portal
            .request(identity(pid), access, 100, 0)
            .unwrap()
    }
    #[test]
    fn consent_keeps_exact_selected_inode_through_rename_unlink_and_recreate() {
        for unlink in [false, true] {
            let mut broker = Broker::new();
            let mut fs = Model::new();
            let owner = identity(1);
            let id = request(&mut broker, 1, Access::ReadWrite);
            approve_with(&mut broker, &mut fs, id, "/native/a", Access::ReadOnly, 1).unwrap();
            if unlink {
                fs.files.remove(&mut fs.disk, "/native/a").unwrap();
            } else {
                fs.files
                    .rename(&mut fs.disk, "/native/a", "/native/renamed")
                    .unwrap();
            }
            fs.files
                .replace(&mut fs.disk, "/native/a", b"replacement")
                .unwrap();
            let (file, access) = consume_with(&mut broker, &mut fs, owner, id, 2).unwrap();
            assert_eq!(access, Rights::READ);
            assert!(!access.contains(Rights::WRITE));
            let mut bytes = [0; 8];
            assert_eq!(fs.files.read(file, &mut bytes), Ok(8));
            assert_eq!(&bytes, b"selected");
            assert_eq!(fs.opens, 1);
            assert_eq!(
                broker.portal.state(id, owner, 2),
                Err(Error::InvalidRequest)
            );
            assert!(broker.held.iter().all(Option::is_none));
            fs.close(file);
            assert_eq!(fs.live, 0);
        }
    }
    #[test]
    fn attenuation_existence_and_allocation_failures_preserve_pending_without_leaks() {
        let mut broker = Broker::new();
        let mut fs = Model::new();
        let owner = identity(1);
        let id = request(&mut broker, 1, Access::ReadOnly);
        assert_eq!(
            approve_with(&mut broker, &mut fs, id, "/native/a", Access::ReadWrite, 1),
            Err(Status::AccessDenied)
        );
        assert_eq!(fs.opens, 0);
        assert_eq!(
            approve_with(
                &mut broker,
                &mut fs,
                id,
                "/native/missing",
                Access::ReadOnly,
                1
            ),
            Err(Status::NotFound)
        );
        assert_eq!(
            approve_with(
                &mut broker,
                &mut fs,
                id,
                "/native/a/../b",
                Access::ReadOnly,
                1
            ),
            Err(Status::InvalidArgument)
        );
        fs.fail = true;
        assert_eq!(
            approve_with(&mut broker, &mut fs, id, "/native/a", Access::ReadOnly, 1),
            Err(Status::NoMemory)
        );
        assert_eq!(broker.portal.state(id, owner, 1), Ok(State::Pending));
        assert_eq!(fs.live, 0);
        fs.fail = false;
        approve_with(&mut broker, &mut fs, id, "/native/a", Access::ReadOnly, 2).unwrap();
        // A full recipient capability table performs only this nondestructive
        // peek, never consumes or installs. Its held reference later expires.
        let choice = approval_with(&mut broker, &mut fs, owner, id, 2).unwrap();
        assert_eq!(rights(choice.access()), Rights::READ);
        assert_eq!(fs.live, 1);
        expire_with(&mut broker, &mut fs, 100).unwrap();
        assert_eq!(fs.live, 0);
        assert_eq!(
            approval_with(&mut broker, &mut fs, owner, id, 100).err(),
            Some(Status::TimedOut)
        );
        assert_eq!(
            approval_with(&mut broker, &mut fs, owner, id, 100).err(),
            Some(Status::InvalidHandle)
        );
    }
    #[test]
    fn cancel_expiry_death_and_denial_close_references_and_reclaim_request_budget() {
        let mut broker = Broker::new();
        let mut fs = Model::new();
        let owner = identity(1);
        let id = request(&mut broker, 1, Access::ReadWrite);
        approve_with(&mut broker, &mut fs, id, "/native/a", Access::ReadWrite, 1).unwrap();
        cancel_with(&mut broker, &mut fs, owner, id, 2).unwrap();
        assert_eq!(fs.live, 0);
        assert_eq!(broker.portal.state(id, owner, 2), Ok(State::Cancelled));
        assert_eq!(
            approval_with(&mut broker, &mut fs, owner, id, 2).err(),
            Some(Status::Cancelled)
        );
        assert_eq!(
            approval_with(&mut broker, &mut fs, owner, id, 2).err(),
            Some(Status::InvalidHandle)
        );
        let id = broker
            .portal
            .request(owner, Access::ReadWrite, 100, 2)
            .unwrap();
        approve_with(&mut broker, &mut fs, id, "/native/a", Access::ReadWrite, 2).unwrap();
        let impostor = ProcessIdentity::new(1, 2, [1; 32]).unwrap();
        died_with(&mut broker, &mut fs, impostor);
        assert_eq!(fs.live, 1);
        died_with(&mut broker, &mut fs, owner);
        assert_eq!(fs.live, 0);
        assert_eq!(
            broker.portal.state(id, owner, 2),
            Err(Error::InvalidRequest)
        );
        let id = broker
            .portal
            .request(owner, Access::ReadOnly, 100, 2)
            .unwrap();
        broker.portal.deny(id, owner, 2).unwrap();
        assert_eq!(
            approval_with(&mut broker, &mut fs, owner, id, 2).err(),
            Some(Status::AccessDenied)
        );
        assert!(broker
            .portal
            .request(owner, Access::ReadOnly, 100, 2)
            .is_ok());
        assert_eq!(fs.closed, 2);
    }
    #[test]
    fn four_held_slots_and_wrong_recipient_never_transfer_or_leak_approvals() {
        let mut broker = Broker::new();
        let mut fs = Model::new();
        for pid in 1..=4 {
            let id = broker
                .portal
                .request(identity(pid), Access::ReadWrite, 100, 1)
                .unwrap();
            approve_with(&mut broker, &mut fs, id, "/native/a", Access::ReadWrite, 1).unwrap();
        }
        assert_eq!(fs.live, 4);
        assert_eq!(
            broker.portal.request(identity(5), Access::ReadOnly, 100, 1),
            Err(Error::Full)
        );
        let held = broker.held[0].unwrap();
        assert_eq!(
            consume_with(&mut broker, &mut fs, identity(2), held.id, 2).err(),
            Some(Status::AccessDenied)
        );
        assert_eq!(fs.live, 4);
        expire_with(&mut broker, &mut fs, 100).unwrap();
        assert_eq!(fs.live, 0);
        assert_eq!(fs.closed, 4);
        assert_eq!(broker.portal.pending(100).unwrap().count(), 0);
        assert!(broker.held.iter().all(Option::is_none));
    }
    #[test]
    fn typed_access_and_absolute_deadline_limits_do_not_wrap() {
        assert_eq!(access(0), Err(Status::InvalidArgument));
        assert_eq!(access(2), Err(Status::InvalidArgument));
        assert_eq!(access(7), Err(Status::InvalidArgument));
        assert!(!deadline_valid(10, 10));
        assert!(!deadline_valid(9, 10));
        assert!(deadline_valid(30_000_000_010, 10));
        assert!(!deadline_valid(30_000_000_011, 10));
        assert!(!deadline_valid(0, u64::MAX));
    }
}
