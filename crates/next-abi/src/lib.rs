#![no_std]
//! Experimental native ABI and validated boot/executable input contracts.
//! Numeric values are development ABI v0, not a frozen Next 1.0 compatibility promise.

pub mod boot;
pub mod client_close;
pub mod client_input;
pub mod dns_lookup;
pub mod elf;
pub mod initrd;
pub mod kernel;
pub mod native;
pub mod pe;
pub mod service;
pub mod surface;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(pub u64);
impl Rights {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXECUTE: Self = Self(4);
    pub const DUPLICATE: Self = Self(8);
    pub const TRANSFER: Self = Self(16);
    pub const WAIT: Self = Self(32);
    pub const SIGNAL: Self = Self(64);
    pub const ADMIN: Self = Self(128);
    pub const KNOWN: Self = Self(255);
    pub const fn contains(self, requested: Self) -> bool {
        self.0 & requested.0 == requested.0
    }
    pub const fn is_valid(self) -> bool {
        self.0 & !Self::KNOWN.0 == 0
    }
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Success = 0,
    InvalidArgument = -1,
    AccessDenied = -2,
    InvalidHandle = -3,
    NotFound = -4,
    NoMemory = -5,
    NotSupported = -6,
    WouldBlock = -7,
    TimedOut = -8,
    BufferTooSmall = -9,
    IoError = -10,
    Faulted = -11,
    PeerClosed = -12,
    Cancelled = -13,
}

/// x86-64 development syscall convention: rax = number/result, arguments in
/// rdi, rsi, rdx, r10, r8, r9; rcx and r11 clobbered. Unknown calls return
/// NotSupported. Defining a number does not imply an implemented kernel call.
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syscall {
    Exit = 0,
    LogWrite = 1,
    HandleClose = 2,
    HandleDuplicate = 3,
    ObjectWait = 4,
    ChannelSend = 5,
    ChannelReceive = 6,
    MemoryMap = 7,
    MemoryUnmap = 8,
    ProcessSpawn = 9,
    ThreadYield = 10,
    ClockRead = 11,
    ChannelCreate = 12,
    ProcessKill = 13,
    ThreadSleep = 14,
    SystemReady = 15,
    FileOpen = 16,
    FileRead = 17,
    FileWrite = 18,
    FileSeek = 19,
    FileStat = 20,
    EventCreate = 21,
    EventSignal = 22,
    EventReset = 23,
    FileRename = 24,
    FileUnlink = 25,
    ThreadSetTls = 26,
    ThreadGetTls = 27,
    ClockRealtime = 28,
    PortalRequestOpen = 29,
    PortalPoll = 30,
    PortalCancel = 31,
    ServiceConfiguration = 32,
    DatagramCreate = 33,
    DatagramSend = 34,
    DatagramReceive = 35,
    LookupCreate = 36,
    LookupResult = 37,
    SurfaceCreate = 38,
    SurfaceConfigure = 39,
    SurfaceAcknowledge = 40,
    SurfaceBufferImport = 41,
    SurfacePublish = 42,
    SurfaceInputRead = 43,
    SurfaceCloseReply = 44,
    /// Bounded atomic wait-any over native event handles (v0 extension).
    ObjectWaitMany = 45,
}

/// Maximum input count for the native `ObjectWaitMany` event wait-any call.
pub const OBJECT_WAIT_MANY_MAX: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rights_fail_closed() {
        assert!(Rights::READ.union(Rights::WRITE).contains(Rights::READ));
        assert!(!Rights::READ.contains(Rights::ADMIN));
        assert!(!Rights(u64::MAX).is_valid());
    }
    #[test]
    fn wait_many_is_a_distinct_native_extension_number_with_fixed_bound() {
        assert_eq!(Syscall::ObjectWaitMany as u64, 45);
        assert_eq!(OBJECT_WAIT_MANY_MAX, 16);
    }
}

/// Native UTC/monotonic time representation; nanoseconds are in 0..1_000_000_000.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timespec {
    pub seconds: i64,
    pub nanoseconds: i64,
}
