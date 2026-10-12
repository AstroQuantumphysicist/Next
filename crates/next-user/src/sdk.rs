//! Typed helpers for the experimental Next native ABI, without POSIX emulation.
//! Handles are borrowed tokens: copying one neither duplicates nor transfers
//! authority. Close and successful IPC transfer invalidate every local copy.
use crate::{call, Status, Syscall};
pub use next_abi::Rights;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Status(Status),
    /// Preserve an unknown negative status instead of interpreting it as data.
    UnknownStatus(i64),
    InvalidResult,
}
pub type Result<T> = core::result::Result<T, Error>;
/// Decode status separately from signed process exit results (ObjectWait writes
/// those to a distinct output slot). All nonnegative values remain data.
pub fn decode(value: i64) -> Result<u64> {
    let status = match value {
        v if v >= 0 => return Ok(v as u64),
        -1 => Status::InvalidArgument,
        -2 => Status::AccessDenied,
        -3 => Status::InvalidHandle,
        -4 => Status::NotFound,
        -5 => Status::NoMemory,
        -6 => Status::NotSupported,
        -7 => Status::WouldBlock,
        -8 => Status::TimedOut,
        -9 => Status::BufferTooSmall,
        -10 => Status::IoError,
        -11 => Status::Faulted,
        -12 => Status::PeerClosed,
        -13 => Status::Cancelled,
        other => return Err(Error::UnknownStatus(other)),
    };
    Err(Error::Status(status))
}
fn unit(value: i64) -> Result<()> {
    if decode(value)? == 0 {
        Ok(())
    } else {
        Err(Error::InvalidResult)
    }
}
fn count(value: i64, limit: usize) -> Result<usize> {
    let n = decode(value)?;
    if n > limit as u64 {
        Err(Error::InvalidResult)
    } else {
        Ok(n as usize)
    }
}
fn invalid<T>() -> Result<T> {
    Err(Error::Status(Status::InvalidArgument))
}
fn rights_valid(rights: Rights) -> Result<()> {
    if rights.is_valid() {
        Ok(())
    } else {
        invalid()
    }
}
/// An immutable absolute ASCII namespace path, bounded to the supported
/// 31-byte file namespace. No ambient current directory is implied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Path {
    bytes: [u8; 32],
    len: usize,
}
impl Path {
    pub fn new(text: &str) -> Result<Self> {
        if text.len() < 2
            || text.len() > 31
            || !text.starts_with('/')
            || !text.bytes().all(|b| b.is_ascii() && !b.is_ascii_control())
            || text
                .split('/')
                .skip(1)
                .any(|c| c.is_empty() || c == "." || c == "..")
        {
            return invalid();
        }
        let mut result = Self {
            bytes: [0; 32],
            len: text.len(),
        };
        result.bytes[..text.len()].copy_from_slice(text.as_bytes());
        Ok(result)
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
    fn ptr(&self) -> u64 {
        self.bytes.as_ptr() as u64
    }
}
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(u64);
impl Handle {
    /// The kernel validates object identity, generation and rights on each call.
    pub fn from_raw(raw: u64) -> Result<Self> {
        if raw == 0 {
            invalid()
        } else {
            Ok(Self(raw))
        }
    }
    pub fn raw(self) -> u64 {
        self.0
    }
    fn returned(value: i64) -> Result<Self> {
        Self::from_raw(decode(value)?)
    }
    pub fn close(self) -> Result<()> {
        unit(call(Syscall::HandleClose, [self.0, 0, 0, 0, 0, 0]))
    }
    pub fn duplicate(self, rights: Rights) -> Result<Self> {
        rights_valid(rights)?;
        Self::returned(call(
            Syscall::HandleDuplicate,
            [self.0, rights.0, 0, 0, 0, 0],
        ))
    }
    /// Nonblocking process/event wait. Exited processes retain their signed
    /// result; auto-reset events consume a signal only on successful copyout.
    pub fn poll(self) -> Result<i64> {
        self.wait(Wait::Poll)
    }
    pub fn wait(self, mode: Wait) -> Result<i64> {
        let mut output = 0i64;
        let (flags, deadline) = wait_arguments(mode, false);
        unit(call(
            Syscall::ObjectWait,
            [
                self.0,
                &mut output as *mut i64 as u64,
                flags,
                deadline,
                0,
                0,
            ],
        ))?;
        Ok(output)
    }
    /// Atomically wait for any event in `handles` and return its input index.
    /// The kernel validates every handle and output buffer before consuming a
    /// signal; at most `OBJECT_WAIT_MANY_MAX` unique event objects are allowed.
    pub fn wait_many(handles: &[Self], mode: Wait) -> Result<usize> {
        if handles.is_empty() || handles.len() > next_abi::OBJECT_WAIT_MANY_MAX {
            return invalid();
        }
        let mut raw = [0u64; next_abi::OBJECT_WAIT_MANY_MAX];
        for (slot, handle) in handles.iter().enumerate() {
            raw[slot] = handle.raw();
        }
        let mut selected = u64::MAX;
        let (flags, deadline) = wait_arguments(mode, false);
        unit(call(
            Syscall::ObjectWaitMany,
            [
                raw.as_ptr() as u64,
                handles.len() as u64,
                &mut selected as *mut u64 as u64,
                flags,
                deadline,
                0,
            ],
        ))?;
        if selected >= handles.len() as u64 {
            return Err(Error::InvalidResult);
        }
        Ok(selected as usize)
    }
    pub fn log(self, text: &str) -> Result<usize> {
        count(crate::log(self.0, text), text.len())
    }
    pub fn kill(self, exit_code: i64) -> Result<()> {
        unit(crate::kill(self.0, exit_code))
    }
    pub fn read(self, bytes: &mut [u8]) -> Result<usize> {
        count(crate::file_read(self.0, bytes), bytes.len())
    }
    pub fn write(self, bytes: &[u8]) -> Result<usize> {
        count(crate::file_write(self.0, bytes), bytes.len())
    }
    pub fn seek(self, offset: i64, origin: Seek) -> Result<u64> {
        decode(crate::file_seek(self.0, offset, origin as u64))
    }
    pub fn stat(self) -> Result<FileInfo> {
        let mut info = [0; 2];
        unit(crate::file_stat(self.0, &mut info))?;
        Ok(FileInfo {
            length: info[0],
            offset: info[1],
        })
    }
    pub fn signal(self) -> Result<()> {
        unit(crate::event_signal(self.0))
    }
    pub fn reset(self) -> Result<()> {
        unit(crate::event_reset(self.0))
    }
    pub fn receive(self, bytes: &mut [u8]) -> Result<usize> {
        count(crate::receive(self.0, bytes), bytes.len())
    }
    /// On success the kernel moves every attachment listed in the wire frame;
    /// source tokens become stale. On failure all attachments remain owned.
    pub fn send(self, frame: &[u8], rights: &[Rights]) -> Result<()> {
        let decoded = helix_core::ipc::decode(frame, 256, 4, 0)
            .map_err(|_| Error::Status(Status::InvalidArgument))?;
        if decoded.handle_count() != rights.len() || rights.len() > 4 {
            return invalid();
        }
        let mut wire_rights = [0; 4];
        for (i, r) in rights.iter().enumerate() {
            rights_valid(*r)?;
            wire_rights[i] = r.0;
        }
        unit(crate::send_with_rights(
            self.0,
            frame,
            &wire_rights[..rights.len()],
        ))
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Wait {
    Poll,
    Block,
    Until(u64),
}
fn wait_arguments(mode: Wait, writable: bool) -> (u64, u64) {
    let (flags, deadline) = match mode {
        Wait::Poll => (0, 0),
        Wait::Block => (1, 0),
        Wait::Until(deadline) => (1, deadline),
    };
    (flags | if writable { 2 } else { 0 }, deadline)
}
#[repr(u64)]
#[derive(Clone, Copy, Debug)]
pub enum Seek {
    Start = 0,
    Current = 1,
    End = 2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub length: u64,
    pub offset: u64,
}
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct OpenFlags(u64);
impl OpenFlags {
    pub const EXISTING: Self = Self(0);
    pub const CREATE: Self = Self(1);
    pub const CREATE_TRUNCATE: Self = Self(3);
    pub const TRUNCATE: Self = Self(2);
}
/// Launcher delegation is explicit and grants the child's complete native
/// launcher authority. It is never enabled by the ordinary spawn default.
pub const SPAWN_INHERIT_LAUNCHER: u64 = 1;
#[repr(u64)]
#[derive(Clone, Copy, Debug)]
pub enum LauncherInheritance {
    None = 0,
    Delegate = SPAWN_INHERIT_LAUNCHER,
}
#[derive(Clone, Copy, Debug)]
pub struct Launcher(Handle);
impl Launcher {
    pub fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }
    pub fn spawn(
        self,
        path: &Path,
        endpoint: Option<Handle>,
        inheritance: LauncherInheritance,
    ) -> Result<Handle> {
        Handle::returned(call(
            Syscall::ProcessSpawn,
            [
                self.0 .0,
                path.ptr(),
                path.len as u64,
                endpoint.map_or(0, Handle::raw),
                inheritance as u64,
                0,
            ],
        ))
    }
    pub fn open(self, path: &Path, flags: OpenFlags, rights: Rights) -> Result<Handle> {
        rights_valid(rights)?;
        Handle::returned(call(
            Syscall::FileOpen,
            [self.0 .0, path.ptr(), path.len as u64, flags.0, rights.0, 0],
        ))
    }
    pub fn rename(self, old: &Path, new: &Path) -> Result<()> {
        unit(call(
            Syscall::FileRename,
            [
                self.0 .0,
                old.ptr(),
                old.len as u64,
                new.ptr(),
                new.len as u64,
                0,
            ],
        ))
    }
    pub fn unlink(self, path: &Path) -> Result<()> {
        unit(call(
            Syscall::FileUnlink,
            [self.0 .0, path.ptr(), path.len as u64, 0, 0, 0],
        ))
    }
    pub fn system_ready(self) -> Result<()> {
        unit(crate::ready(self.0 .0))
    }
}
/// A scoped file namespace token, with no process-launch or system-ready API.
/// Source11 trusted development `runfiles` places this token in the legacy
/// `StartInfo.launcher` field. Constructing a wrapper grants no authority: the
/// kernel checks the real object role, rights and authenticated permission grant.
#[derive(Clone, Copy, Debug)]
pub struct FileNamespace(Handle);
impl FileNamespace {
    pub fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }
    pub fn open(self, path: &Path, flags: OpenFlags, rights: Rights) -> Result<Handle> {
        Launcher(self.0).open(path, flags, rights)
    }
    pub fn rename(self, old: &Path, new: &Path) -> Result<()> {
        Launcher(self.0).rename(old, new)
    }
    pub fn unlink(self, path: &Path) -> Result<()> {
        Launcher(self.0).unlink(path)
    }
}
pub fn channel_pair() -> Result<[Handle; 2]> {
    let mut pair = [0; 2];
    unit(crate::channel_pair(&mut pair))?;
    Ok([Handle::from_raw(pair[0])?, Handle::from_raw(pair[1])?])
}
pub fn event(manual: bool, signaled: bool, rights: Rights) -> Result<Handle> {
    rights_valid(rights)?;
    Handle::returned(crate::event_create(
        u64::from(manual) | (u64::from(signaled) << 1),
        rights,
    ))
}
pub fn clock() -> Result<u64> {
    decode(crate::clock())
}
/// UTC based on a validated RTC baseline plus monotonic elapsed time. The
/// native status reports unavailable RTC; no time is synthesized by the SDK.
pub fn clock_realtime() -> Result<crate::Timespec> {
    let mut output = crate::Timespec::default();
    unit(crate::clock_realtime(&mut output))?;
    if !(0..1_000_000_000).contains(&output.nanoseconds) {
        return Err(Error::InvalidResult);
    }
    Ok(output)
}
pub fn yield_now() -> Result<()> {
    unit(call(Syscall::ThreadYield, [0; 6]))
}
pub fn sleep_until(deadline: u64) -> Result<()> {
    unit(call(Syscall::ThreadSleep, [deadline, 0, 0, 0, 0, 0]))
}
pub fn tls_fs() -> Result<u64> {
    let mut base = 0u64;
    unit(call(
        Syscall::ThreadGetTls,
        [&mut base as *mut u64 as u64, 0, 0, 0, 0, 0],
    ))?;
    Ok(base)
}
/// Install a native per-process FS base, preserved across yields and IRQs.
///
/// # Safety
/// The caller must keep the referenced storage alive and ensure changing FS
/// does not invalidate any thread-local assumptions of linked code. Zero clears.
pub unsafe fn set_tls_fs(base: u64) -> Result<()> {
    unit(call(Syscall::ThreadSetTls, [base, 0, 0, 0, 0, 0]))
}
/// Map anonymous pages. Protection bits are READ=1, WRITE=2, EXECUTE=4; W+X
/// is denied. Returns an address, without creating a Rust reference or owner.
pub fn map(length: usize, protection: u64) -> Result<u64> {
    decode(call(
        Syscall::MemoryMap,
        [0, length as u64, protection, 0, 0, 0],
    ))
}
/// # Safety
/// The caller must ensure no references or execution remain in the range.
pub unsafe fn unmap(address: u64, length: usize) -> Result<()> {
    unit(call(
        Syscall::MemoryUnmap,
        [address, length as u64, 0, 0, 0, 0],
    ))
}
pub fn exit(code: i64) -> ! {
    crate::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_native_status_and_unknown_values_are_decoded_without_exit_collision() {
        let statuses = [
            Status::InvalidArgument,
            Status::AccessDenied,
            Status::InvalidHandle,
            Status::NotFound,
            Status::NoMemory,
            Status::NotSupported,
            Status::WouldBlock,
            Status::TimedOut,
            Status::BufferTooSmall,
            Status::IoError,
            Status::Faulted,
            Status::PeerClosed,
            Status::Cancelled,
        ];
        for status in statuses {
            assert_eq!(decode(status as i64), Err(Error::Status(status)));
        }
        assert_eq!(decode(-99), Err(Error::UnknownStatus(-99)));
        assert_eq!(decode(0), Ok(0));
        assert_eq!(decode(i64::MAX), Ok(i64::MAX as u64));
        assert_eq!(unit(7), Err(Error::InvalidResult));
        assert_eq!(count(9, 8), Err(Error::InvalidResult));
    }
    #[test]
    fn wait_many_rejects_empty_and_overlong_slices_before_syscall() {
        let handle = Handle::from_raw(1).unwrap();
        assert_eq!(
            Handle::wait_many(&[], Wait::Poll),
            Err(Error::Status(Status::InvalidArgument))
        );
        let handles = [handle; next_abi::OBJECT_WAIT_MANY_MAX + 1];
        assert_eq!(
            Handle::wait_many(&handles, Wait::Poll),
            Err(Error::Status(Status::InvalidArgument))
        );
    }
    #[test]
    fn file_namespace_rejects_unknown_rights_without_entering_the_gate() {
        let namespace = FileNamespace::from_handle(Handle::from_raw(1).unwrap());
        let path = Path::new("/native/doc").unwrap();
        assert_eq!(
            namespace.open(&path, OpenFlags::EXISTING, Rights(256)),
            Err(Error::Status(Status::InvalidArgument))
        );
    }
    #[test]
    fn paths_copy_input_and_reject_ambiguous_or_overlong_names() {
        let mut input = *b"/native/doc";
        let p = Path::new(core::str::from_utf8(&input).unwrap()).unwrap();
        input[8] = b'x';
        assert_eq!(p.as_str(), "/native/doc");
        for text in [
            "",
            "/",
            "relative",
            "/native//doc",
            "/native/../doc",
            "/./a",
            "/a\n",
            "/é",
            "/1234567890123456789012345678901x",
        ] {
            assert_eq!(Path::new(text), Err(Error::Status(Status::InvalidArgument)));
        }
        assert!(Path::new("/123456789012345678901234567890").is_ok());
    }
    #[test]
    fn malformed_tokens_and_unknown_authority_bits_fail_before_syscalls() {
        assert_eq!(
            Handle::from_raw(0),
            Err(Error::Status(Status::InvalidArgument))
        );
        // Opaque token bits are interpreted only by the kernel.
        assert!(Handle::from_raw(u64::MAX).is_ok());
        assert!(Handle::from_raw(1).is_ok());
        assert!(rights_valid(Rights::READ.union(Rights::TRANSFER)).is_ok());
        assert_eq!(
            rights_valid(Rights(256)),
            Err(Error::Status(Status::InvalidArgument))
        );
        let h = Handle::from_raw(1).unwrap();
        assert_eq!(
            h.send(&[], &[]),
            Err(Error::Status(Status::InvalidArgument))
        );
        assert_eq!(
            h.send(&crate::ipc_frame(false), &[Rights::WRITE]),
            Err(Error::Status(Status::InvalidArgument))
        );
    }
}

/// Requested authority for a trusted file chooser. A request carries no path;
/// the trusted UI selects the file and the kernel grants only this access.
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalAccess {
    ReadOnly = 1,
    ReadWrite = 3,
}
/// An owner-bound request identity, distinct from a capability handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileRequest(u64);
impl FileRequest {
    pub fn raw(self) -> u64 {
        self.0
    }
    /// Poll for the selected scoped file capability. Pending requests return
    /// WouldBlock; denial and expiration remain explicit native errors.
    pub fn poll(self) -> Result<Handle> {
        Handle::returned(crate::portal_poll(self.0))
    }
    pub fn cancel(self) -> Result<()> {
        unit(crate::portal_cancel(self.0))
    }
}
/// Queue a chooser request with an absolute monotonic deadline. The kernel
/// bounds the request lifetime to thirty seconds and authenticates ownership.
pub fn request_file(access: PortalAccess, deadline: u64) -> Result<FileRequest> {
    let id = decode(crate::portal_request_open(access as u64, deadline))?;
    if id == 0 {
        return Err(Error::InvalidResult);
    }
    Ok(FileRequest(id))
}

/// Retrieve configuration authenticated as a member of the running PID1
/// package. Other principals cannot retrieve or override this configuration.
pub fn service_configuration(output: &mut [u8]) -> Result<usize> {
    if output.is_empty() || output.len() > next_abi::service::MAX_BYTES {
        return invalid();
    }
    count(crate::service_configuration(output), output.len())
}

/// Maximum unfragmented IPv4 payload in the connected development UDP profile.
pub const DATAGRAM_MAX_PAYLOAD: usize = 1472;
pub const DATAGRAM_FIRST_PORT: u16 = 49200;
pub const DATAGRAM_PORTS: u16 = 8;
const DATAGRAM_RIGHTS: u64 = 0x3b;
fn datagram_options(local_port: u16, rights: Rights) -> Result<()> {
    if (local_port != 0
        && !(DATAGRAM_FIRST_PORT..DATAGRAM_FIRST_PORT + DATAGRAM_PORTS).contains(&local_port))
        || rights.0 & !DATAGRAM_RIGHTS != 0
    {
        invalid()
    } else {
        Ok(())
    }
}
/// Borrowed typed socket token. Copying this value neither duplicates nor
/// transfers authority; close invalidates all local copies. The kernel checks
/// object kind, rights, generation and authenticated NETWORK grants on use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Datagram(Handle);
impl Datagram {
    /// Connect to the trusted development-policy peer; callers cannot set a
    /// destination. Port zero requests an available development ephemeral port.
    /// A signed NETWORK request alone does not authorize socket creation.
    pub fn create(local_port: u16, rights: Rights) -> Result<Self> {
        datagram_options(local_port, rights)?;
        Handle::returned(call(
            Syscall::DatagramCreate,
            [local_port as u64, rights.0, 0, 0, 0, 0],
        ))
        .map(Self)
    }
    /// Wrapping an IPC-received token grants no authority or object identity;
    /// the kernel checks both when the wrapper is used.
    pub fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }
    pub fn handle(self) -> Handle {
        self.0
    }
    /// Nonblocking queue acceptance, not remote delivery acknowledgement.
    /// Zero-length UDP datagrams are valid and return zero on acceptance.
    pub fn send(self, payload: &[u8]) -> Result<usize> {
        if payload.len() > DATAGRAM_MAX_PAYLOAD {
            return invalid();
        }
        count(
            call(
                Syscall::DatagramSend,
                [
                    self.0.raw(),
                    payload.as_ptr() as u64,
                    payload.len() as u64,
                    0,
                    0,
                    0,
                ],
            ),
            payload.len(),
        )
    }
    /// Receive one whole datagram. Short/invalid output preserves the queued
    /// packet; WouldBlock is distinct from a received empty datagram (zero).
    pub fn receive(self, output: &mut [u8]) -> Result<usize> {
        if output.len() > DATAGRAM_MAX_PAYLOAD {
            return invalid();
        }
        count(
            call(
                Syscall::DatagramReceive,
                [
                    self.0.raw(),
                    output.as_mut_ptr() as u64,
                    output.len() as u64,
                    0,
                    0,
                    0,
                ],
            ),
            output.len(),
        )
    }
    pub fn close(self) -> Result<()> {
        self.0.close()
    }
    pub fn duplicate(self, rights: Rights) -> Result<Self> {
        datagram_options(0, rights)?;
        self.0.duplicate(rights).map(Self)
    }
    /// Level-triggered readable polling. Successful readiness returns zero;
    /// it does not dequeue a packet. WAIT authority remains independently checked.
    pub fn poll_readable(self) -> Result<()> {
        self.wait_readable(Wait::Poll)
    }
    /// Wait for level-readable readiness without consuming a queued datagram.
    /// Until uses an absolute monotonic deadline; timeout preserves the queue.
    pub fn wait_readable(self, mode: Wait) -> Result<()> {
        self.wait_ready(mode, false)
    }
    /// Poll for transmit queue capacity. Success is neither a remote delivery
    /// acknowledgement nor a guarantee a subsequent send cannot WouldBlock.
    pub fn poll_writable(self) -> Result<()> {
        self.wait_writable(Wait::Poll)
    }
    /// Wait for level-writable readiness with WAIT and NETWORK authority.
    /// Driver/lease errors complete the wait; copying the token grants no rights.
    pub fn wait_writable(self, mode: Wait) -> Result<()> {
        self.wait_ready(mode, true)
    }
    fn wait_ready(self, mode: Wait, writable: bool) -> Result<()> {
        let mut output = 0i64;
        let (flags, deadline) = wait_arguments(mode, writable);
        unit(call(
            Syscall::ObjectWait,
            [
                self.0.raw(),
                &mut output as *mut i64 as u64,
                flags,
                deadline,
                0,
                0,
            ],
        ))?;
        if output == 0 {
            Ok(())
        } else {
            Err(Error::InvalidResult)
        }
    }
}

pub use next_abi::dns_lookup::{Family as LookupFamily, LookupResult};

/// Borrowed capability for an immutable asynchronous resolver result. The
/// kernel's trusted resolver policy fixes the server and independently checks
/// signed NETWORK declarations and execution grants on every use/import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lookup(Handle);
impl Lookup {
    pub fn begin(
        name: &str,
        family: LookupFamily,
        rights: Rights,
        deadline: Option<u64>,
    ) -> Result<Self> {
        if !next_abi::dns_lookup::canonical_name(name)
            || rights.0 & !next_abi::dns_lookup::LOOKUP_RIGHTS != 0
        {
            return invalid();
        }
        Handle::returned(call(
            Syscall::LookupCreate,
            [
                name.as_ptr() as u64,
                name.len() as u64,
                family as u64,
                rights.0,
                deadline.unwrap_or(0),
                0,
            ],
        ))
        .map(Self)
    }
    pub fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }
    pub fn handle(self) -> Handle {
        self.0
    }
    pub fn duplicate(self, rights: Rights) -> Result<Self> {
        if rights.0 & !next_abi::dns_lookup::LOOKUP_RIGHTS != 0 {
            return invalid();
        }
        self.0.duplicate(rights).map(Self)
    }
    pub fn close(self) -> Result<()> {
        self.0.close()
    }
    pub fn wait(self, mode: Wait) -> Result<()> {
        let mut output = 0i64;
        let (flags, deadline) = wait_arguments(mode, false);
        unit(call(
            Syscall::ObjectWait,
            [
                self.0.raw(),
                &mut output as *mut i64 as u64,
                flags,
                deadline,
                0,
                0,
            ],
        ))?;
        if output == 0 {
            Ok(())
        } else {
            Err(Error::InvalidResult)
        }
    }
    /// Results remain repeatable until last close. TTL describes the completed
    /// answer/cache observation; keeping this token does not extend DNS validity.
    pub fn result(self) -> Result<LookupResult> {
        let mut bytes = [0; next_abi::dns_lookup::RESULT_BYTES];
        if decode(call(
            Syscall::LookupResult,
            [
                self.0.raw(),
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
                0,
                0,
                0,
            ],
        ))? != bytes.len() as u64
        {
            return Err(Error::InvalidResult);
        }
        LookupResult::decode(&bytes).ok_or(Error::InvalidResult)
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;
    #[test]
    fn malformed_names_and_excess_authority_fail_before_syscall() {
        for name in ["", "Next.test", "a..b", "next.test.", "a/b"] {
            assert_eq!(
                Lookup::begin(name, LookupFamily::A, Rights::READ, None),
                Err(Error::Status(Status::InvalidArgument))
            );
        }
        for rights in [
            Rights::WRITE,
            Rights::EXECUTE,
            Rights::SIGNAL,
            Rights::ADMIN,
        ] {
            assert_eq!(
                Lookup::begin("next.test", LookupFamily::A, rights, None),
                Err(Error::Status(Status::InvalidArgument))
            );
        }
        let token = Lookup::from_handle(Handle::from_raw(17).unwrap());
        assert_eq!(
            token.duplicate(Rights::WRITE),
            Err(Error::Status(Status::InvalidArgument))
        );
    }
}

#[cfg(test)]
mod datagram_tests {
    use super::*;
    #[test]
    fn endpoint_and_authority_bounds_fail_before_native_entry() {
        for port in [0, 49200, 49207] {
            assert!(datagram_options(port, Rights::READ.union(Rights::WAIT)).is_ok());
        }
        for port in [1, 53, 49199, 49208, u16::MAX] {
            assert_eq!(
                Datagram::create(port, Rights::READ),
                Err(Error::Status(Status::InvalidArgument))
            );
        }
        for bit in [4, 64, 128, 256, u64::MAX] {
            assert_eq!(
                Datagram::create(0, Rights(bit)),
                Err(Error::Status(Status::InvalidArgument))
            );
        }
        let d = Datagram::from_handle(Handle::from_raw(1).unwrap());
        assert_eq!(d.handle().raw(), 1);
        assert_eq!(
            d.duplicate(Rights::ADMIN),
            Err(Error::Status(Status::InvalidArgument))
        );
    }
    #[test]
    fn directional_wait_modes_preserve_absolute_deadlines() {
        for (mode, read, write, deadline) in [
            (Wait::Poll, 0, 2, 0),
            (Wait::Block, 1, 3, 0),
            (Wait::Until(7), 1, 3, 7),
            (Wait::Until(u64::MAX), 1, 3, u64::MAX),
        ] {
            assert_eq!(wait_arguments(mode, false), (read, deadline));
            assert_eq!(wait_arguments(mode, true), (write, deadline));
        }
    }
    #[test]
    fn oversized_buffers_reject_without_entry_and_empty_counts_are_data() {
        let d = Datagram::from_handle(Handle::from_raw(1).unwrap());
        let mut buffer = [0u8; DATAGRAM_MAX_PAYLOAD + 1];
        assert_eq!(d.send(&buffer), Err(Error::Status(Status::InvalidArgument)));
        assert_eq!(
            d.receive(&mut buffer),
            Err(Error::Status(Status::InvalidArgument))
        );
        assert_eq!(count(0, 0), Ok(0));
        assert_eq!(
            count(Status::WouldBlock as i64, 0),
            Err(Error::Status(Status::WouldBlock))
        );
        assert_eq!(
            count(Status::BufferTooSmall as i64, 1),
            Err(Error::Status(Status::BufferTooSmall))
        );
        assert_eq!(count(2, 1), Err(Error::InvalidResult));
    }
}

pub use next_abi::surface::{Configure as SurfaceConfigure, Frame as SurfaceFrame};
/// Borrowed native surface token. Current backend presents a copied buffer in
/// the trusted kernel-hosted development desktop; this is not display isolation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface(Handle);
fn surface_options(width: u32, height: u32, rights: Rights) -> Result<()> {
    if width == 0
        || height == 0
        || width > next_abi::surface::MAX_DIMENSION
        || height > next_abi::surface::MAX_DIMENSION
        || u64::from(width) * u64::from(height) * 4
            > next_abi::surface::NATIVE_MAX_BUFFER_BYTES as u64
        || rights.0 & !next_abi::surface::SURFACE_RIGHTS != 0
    {
        return invalid();
    }
    Ok(())
}
impl Surface {
    pub fn create(width: u32, height: u32, rights: Rights) -> Result<Self> {
        surface_options(width, height, rights)?;
        Handle::returned(call(
            Syscall::SurfaceCreate,
            [width as u64, height as u64, rights.0, 0, 0, 0],
        ))
        .map(Self)
    }
    pub fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }
    pub fn handle(self) -> Handle {
        self.0
    }
    pub fn close(self) -> Result<()> {
        self.0.close()
    }
    pub fn duplicate(self, rights: Rights) -> Result<Self> {
        if rights.0 & !next_abi::surface::SURFACE_RIGHTS != 0 {
            return invalid();
        }
        self.0.duplicate(rights).map(Self)
    }
    /// Latest configure is repeatable. Retrieval does not acknowledge it.
    pub fn configure(self) -> Result<SurfaceConfigure> {
        let mut bytes = [0; next_abi::surface::CONFIGURE_BYTES];
        if decode(call(
            Syscall::SurfaceConfigure,
            [
                self.0.raw(),
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
                0,
                0,
                0,
            ],
        ))? != bytes.len() as u64
        {
            return Err(Error::InvalidResult);
        }
        SurfaceConfigure::decode(&bytes).map_err(|_| Error::InvalidResult)
    }
    /// Read one complete input record. Short/bad output in the raw ABI preserves
    /// the queued event; this typed wrapper always supplies the exact 64 bytes.
    pub fn read_input(self) -> Result<next_abi::client_input::Event> {
        let mut bytes = [0; next_abi::client_input::EVENT_BYTES];
        if decode(call(
            Syscall::SurfaceInputRead,
            [
                self.0.raw(),
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
                0,
                0,
                0,
            ],
        ))? != bytes.len() as u64
        {
            return Err(Error::InvalidResult);
        }
        next_abi::client_input::Event::decode(&bytes).map_err(|_| Error::InvalidResult)
    }
    /// Explicit negotiated v2 input. Ordinary events use v2 headers too.
    /// A close request is advisory; neither reading nor replying kills a process.
    pub fn read_input_v2(self) -> Result<next_abi::client_close::Event> {
        let mut bytes = [0; next_abi::client_close::EVENT_BYTES];
        if decode(call(
            Syscall::SurfaceInputRead,
            [
                self.0.raw(),
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
                next_abi::client_close::VERSION as u64,
                0,
                0,
            ],
        ))? != bytes.len() as u64
        {
            return Err(Error::InvalidResult);
        }
        next_abi::client_close::Event::decode(&bytes).map_err(|_| Error::InvalidResult)
    }
    /// Reply to the exact pending close identity. WRITE is independently checked
    /// by the kernel. WillClose does not close a surface or terminate a process.
    pub fn reply_close(
        self,
        request: next_abi::client_close::Request,
        response: next_abi::client_close::Response,
    ) -> Result<()> {
        if request.validate().is_err() {
            return invalid();
        }
        unit(call(
            Syscall::SurfaceCloseReply,
            [
                self.0.raw(),
                request.surface_generation,
                request.configure_generation,
                request.ticket,
                request.sequence,
                response as u64,
            ],
        ))
    }
    /// Level-readable input readiness. Readiness does not consume an event,
    /// activate a surface or grant keyboard focus. Independent WAIT required.
    pub fn wait_input(self, mode: Wait) -> Result<()> {
        let mut output = 0i64;
        let (flags, deadline) = wait_arguments(mode, false);
        unit(call(
            Syscall::ObjectWait,
            [
                self.0.raw(),
                &mut output as *mut i64 as u64,
                flags,
                deadline,
                0,
                0,
            ],
        ))?;
        if output == 0 {
            Ok(())
        } else {
            Err(Error::InvalidResult)
        }
    }
    pub fn poll_input(self) -> Result<()> {
        self.wait_input(Wait::Poll)
    }
    pub fn acknowledge(self, configure: SurfaceConfigure) -> Result<()> {
        self.acknowledge_features(configure, 0)
    }
    /// Opt into cooperative v2 close requests for this exact configuration.
    /// A new configuration invalidates negotiation and old close tickets;
    /// callers must acknowledge it again before expecting another close request.
    pub fn acknowledge_close_events(self, configure: SurfaceConfigure) -> Result<()> {
        self.acknowledge_features(configure, next_abi::client_close::FEATURE_CLOSE_REQUEST)
    }
    fn acknowledge_features(self, configure: SurfaceConfigure, features: u64) -> Result<()> {
        if configure.encode().is_err() {
            return invalid();
        }
        unit(call(
            Syscall::SurfaceAcknowledge,
            [
                self.0.raw(),
                configure.surface_generation,
                configure.configure_generation,
                features,
                0,
                0,
            ],
        ))
    }
    /// Queue one structurally valid frame referencing an independently owned
    /// immutable buffer. Queue acceptance is not scanout acknowledgment.
    pub fn publish(self, buffer: SurfaceBuffer, frame: &SurfaceFrame) -> Result<()> {
        if frame.buffer_bytes != buffer.1 as u64 {
            return invalid();
        }
        let mut descriptor = *frame;
        descriptor.buffer_handle = buffer.0.raw();
        let mut bytes = [0; next_abi::surface::MAX_BYTES];
        let length = descriptor
            .encode(&mut bytes)
            .map_err(|_| Error::Status(Status::InvalidArgument))?;
        unit(call(
            Syscall::SurfacePublish,
            [self.0.raw(), bytes.as_ptr() as u64, length as u64, 0, 0, 0],
        ))
    }
}
/// Immutable kernel-owned copy; modifying the imported source slice afterwards
/// does not modify the resource. This wrapper records its known imported size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceBuffer(Handle, usize);
impl SurfaceBuffer {
    pub fn import(bytes: &[u8], rights: Rights) -> Result<Self> {
        if bytes.is_empty()
            || bytes.len() > next_abi::surface::NATIVE_MAX_BUFFER_BYTES
            || rights.0 & !next_abi::surface::BUFFER_RIGHTS != 0
        {
            return invalid();
        }
        Handle::returned(call(
            Syscall::SurfaceBufferImport,
            [bytes.as_ptr() as u64, bytes.len() as u64, rights.0, 0, 0, 0],
        ))
        .map(|handle| Self(handle, bytes.len()))
    }
    pub fn handle(self) -> Handle {
        self.0
    }
    pub fn byte_len(self) -> usize {
        self.1
    }
    pub fn close(self) -> Result<()> {
        self.0.close()
    }
    pub fn duplicate(self, rights: Rights) -> Result<Self> {
        if rights.0 & !next_abi::surface::BUFFER_RIGHTS != 0 {
            return invalid();
        }
        self.0.duplicate(rights).map(|handle| Self(handle, self.1))
    }
}
#[cfg(test)]
mod surface_sdk_tests {
    use super::*;
    #[test]
    fn malformed_requests_fail_before_privileged_gate() {
        for (w, h, rights) in [
            (0, 1, Rights::READ),
            (1, 0, Rights::WRITE),
            (u32::MAX, 1, Rights::READ),
            (8192, 8192, Rights::READ),
            (16, 16, Rights::EXECUTE),
            (16, 16, Rights::TRANSFER),
        ] {
            assert_eq!(
                Surface::create(w, h, rights),
                Err(Error::Status(Status::InvalidArgument))
            );
        }
        assert_eq!(
            SurfaceBuffer::import(&[], Rights::READ),
            Err(Error::Status(Status::InvalidArgument))
        );
        assert_eq!(
            SurfaceBuffer::import(&[1], Rights::WRITE),
            Err(Error::Status(Status::InvalidArgument))
        );
        assert_eq!(
            SurfaceBuffer::import(&[1], Rights::TRANSFER),
            Err(Error::Status(Status::InvalidArgument))
        );
        let s = Surface::from_handle(Handle::from_raw(1).unwrap());
        assert_eq!(
            s.duplicate(Rights::EXECUTE),
            Err(Error::Status(Status::InvalidArgument))
        );
        let buffer = SurfaceBuffer(Handle::from_raw(2).unwrap(), 4);
        let mut frame = SurfaceFrame {
            surface_generation: 1,
            configure_generation: 1,
            frame_generation: 1,
            buffer_handle: 2,
            logical_width: 1,
            logical_height: 1,
            pixel_width: 1,
            pixel_height: 1,
            scale_16_16: 65536,
            stride_bytes: 0,
            buffer_bytes: 4,
            private_content: false,
            format: next_abi::surface::PixelFormat::Bgrx,
            damage_rectangles: [next_abi::surface::Rect::EMPTY; next_abi::surface::MAX_DAMAGE],
            damage_count: 0,
        };
        assert_eq!(
            s.publish(buffer, &frame),
            Err(Error::Status(Status::InvalidArgument))
        );
        frame.stride_bytes = 4;
        frame.buffer_bytes = 8;
        assert_eq!(
            s.publish(buffer, &frame),
            Err(Error::Status(Status::InvalidArgument))
        );
        let configure = SurfaceConfigure {
            surface_generation: 0,
            configure_generation: 1,
            logical_width: 1,
            logical_height: 1,
            pixel_width: 1,
            pixel_height: 1,
            scale_16_16: 65536,
        };
        assert_eq!(
            s.acknowledge(configure),
            Err(Error::Status(Status::InvalidArgument))
        );
    }
}

#[cfg(test)]
mod client_input_sdk_tests {
    use super::*;
    #[test]
    fn surface_wait_authority_is_explicit_and_input_records_do_not_encode_authority() {
        assert_eq!(
            next_abi::surface::SURFACE_RIGHTS,
            Rights::READ
                .union(Rights::WRITE)
                .union(Rights::DUPLICATE)
                .union(Rights::WAIT)
                .0
        );
        assert_eq!(
            surface_options(32, 32, Rights::READ.union(Rights::WAIT)),
            Ok(())
        );
        for rights in [
            Rights::TRANSFER,
            Rights::EXECUTE,
            Rights::SIGNAL,
            Rights::ADMIN,
        ] {
            assert!(surface_options(32, 32, rights).is_err());
        }
        let record = next_abi::client_input::Event {
            surface_generation: 1,
            configure_generation: 2,
            sequence: 3,
            timestamp_ns: 0,
            payload: next_abi::client_input::Payload::FocusGain,
        };
        let bytes = record.encode().unwrap();
        assert_eq!(bytes.len(), 64);
        assert_eq!(next_abi::client_input::Event::decode(&bytes), Ok(record));
    }
}

#[cfg(test)]
mod close_sdk_tests {
    use super::*;
    #[test]
    fn malformed_close_identity_fails_before_native_entry() {
        let surface = Surface::from_handle(Handle::from_raw(1).unwrap());
        let good = next_abi::client_close::Request {
            surface_generation: 1,
            configure_generation: 2,
            sequence: 3,
            ticket: 4,
        };
        for bad in [
            next_abi::client_close::Request {
                surface_generation: 0,
                ..good
            },
            next_abi::client_close::Request {
                configure_generation: 0,
                ..good
            },
            next_abi::client_close::Request {
                sequence: 0,
                ..good
            },
            next_abi::client_close::Request { ticket: 0, ..good },
        ] {
            assert_eq!(
                surface.reply_close(bad, next_abi::client_close::Response::WillClose),
                Err(Error::Status(Status::InvalidArgument))
            );
        }
        let config = SurfaceConfigure {
            surface_generation: 0,
            configure_generation: 1,
            logical_width: 1,
            logical_height: 1,
            pixel_width: 1,
            pixel_height: 1,
            scale_16_16: 65536,
        };
        assert_eq!(
            surface.acknowledge_close_events(config),
            Err(Error::Status(Status::InvalidArgument))
        );
    }
}

#[path = "surface_session.rs"]
mod surface_session;
pub use surface_session::{SurfaceLayout, SurfaceSession};
