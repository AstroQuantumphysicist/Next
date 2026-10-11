//! Connected development UDP capability objects. Runtime authenticates NETWORK4
//! on creation, duplicate, transfer and every use; this adapter independently
//! checks the trusted manifest/grant pair on creation. It never provides a
//! general address constructor or a user-controlled peer allowlist.
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use helix_core::datagram::{self, Datagrams, Endpoint, Grant};
#[path = "network_dns.rs"]
pub(crate) mod dns;
pub use dns::DnsId;
pub use helix_core::datagram::{SocketId, Ticket};
use next_abi::Status;
struct Store(UnsafeCell<Datagrams<8>>);
struct DnsStore(UnsafeCell<dns::Broker>);
unsafe impl Sync for DnsStore {}
static DNS: DnsStore = DnsStore(UnsafeCell::new(dns::Broker::new()));
// Guard-owned static queues. No blocking or process context switch occurs
// while references are live. Device DMA buffers are owned by network runtime.
unsafe impl Sync for Store {}
static STORE: Store = Store(UnsafeCell::new(Datagrams::new()));
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
fn status(error: datagram::Error) -> Status {
    match error {
        datagram::Error::Denied => Status::AccessDenied,
        datagram::Error::Invalid | datagram::Error::TooLarge | datagram::Error::AddressInUse => {
            Status::InvalidArgument
        }
        datagram::Error::Full | datagram::Error::Overflow => Status::NoMemory,
        datagram::Error::Stale => Status::InvalidHandle,
        datagram::Error::WouldBlock => Status::WouldBlock,
        datagram::Error::BufferTooSmall => Status::BufferTooSmall,
    }
}
fn health() -> Result<(), Status> {
    health_status(crate::network::datagram_health())
}
fn health_status(result: Result<(), crate::network::NetworkError>) -> Result<(), Status> {
    match result {
        Ok(()) => Ok(()),
        Err(crate::network::NetworkError::Busy) => Err(Status::WouldBlock),
        Err(crate::network::NetworkError::NotFound) => Err(Status::NotSupported),
        Err(_) => Err(Status::IoError),
    }
}
fn internal_grant() -> Grant {
    // Every socket has this one trusted endpoint. This is not an authorization
    // decision; the runtime principal gate must precede each public use.
    Grant::from_trusted_policy(4, 4, datagram::DEVELOPMENT_PEER).unwrap_or_else(|_| unreachable!())
}
pub fn create(local_port: u16, declared: u64, granted: u64) -> Result<SocketId, Status> {
    let grant = Grant::from_trusted_policy(declared, granted, datagram::DEVELOPMENT_PEER)
        .map_err(status)?;
    let address = crate::network::datagram_address().ok_or(Status::NotSupported)?;
    if address != [10, 0, 2, 15] {
        return Err(Status::NotSupported);
    }
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    if local_port != 0 {
        return store
            .create(
                &grant,
                Endpoint {
                    address,
                    port: local_port,
                },
            )
            .map_err(status);
    }
    for port in datagram::FIRST_PORT..datagram::FIRST_PORT + datagram::MAX_SOCKETS as u16 {
        match store.create(&grant, Endpoint { address, port }) {
            Ok(socket) => return Ok(socket),
            Err(datagram::Error::AddressInUse) => {}
            Err(error) => return Err(status(error)),
        }
    }
    Err(Status::NoMemory)
}
pub fn retain(id: SocketId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }
        .retain(&internal_grant(), id)
        .map_err(status)
}
pub fn close(id: SocketId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }.close(id).map_err(status)
}
pub fn send(id: SocketId, bytes: &[u8]) -> Result<usize, Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    store.ready(&internal_grant(), id).map_err(status)?;
    health()?;
    store.send(&internal_grant(), id, bytes).map_err(status)
}
pub fn peek_receive(id: SocketId, output: &mut [u8]) -> Result<(usize, Ticket), Status> {
    let _guard = Guard::acquire()?;
    let store = unsafe { &mut *STORE.0.get() };
    store.ready(&internal_grant(), id).map_err(status)?;
    health()?;
    store
        .peek_receive(&internal_grant(), id, output)
        .map_err(status)
}
pub fn consume_receive(id: SocketId, ticket: Ticket) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *STORE.0.get() }
        .consume_receive(&internal_grant(), id, ticket)
        .map_err(status)
}
fn readiness(id: SocketId, write: bool) -> Result<bool, Status> {
    let _guard = Guard::acquire()?;
    let ready = unsafe { &*STORE.0.get() }
        .ready(&internal_grant(), id)
        .map_err(status)?;
    health()?;
    Ok(if write { ready.1 } else { ready.0 })
}
pub fn read_ready(id: SocketId) -> Result<bool, Status> {
    readiness(id, false)
}
pub fn write_ready(id: SocketId) -> Result<bool, Status> {
    readiness(id, true)
}
/// Bounded nonblocking device progress. A reentrant caller returns immediately;
/// missing/failed devices retain all permanent DMA ownership and queued bytes.
pub fn pump() {
    let Ok(_guard) = Guard::acquire() else {
        return;
    };
    let _ = crate::network::datagram_pump_dns(unsafe { &mut *STORE.0.get() }, unsafe {
        &mut *DNS.0.get()
    });
}
/// Start an owner-authenticated DNS request. The resolver endpoint is fixed.
pub fn dns_begin(
    host: &str,
    kind: helix_core::dns::Type,
    deadline: u64,
    declared: u64,
    granted: u64,
) -> Result<DnsId, Status> {
    if declared & granted & 4 == 0 {
        return Err(Status::AccessDenied);
    }
    let _guard = Guard::acquire()?;
    health()?;
    unsafe { &mut *DNS.0.get() }.begin(
        host,
        kind,
        deadline,
        declared,
        granted,
        crate::timer::nanoseconds(),
    )
}
pub fn dns_retain(id: DnsId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *DNS.0.get() }.retain(id)
}
pub fn dns_close(id: DnsId) -> Result<(), Status> {
    let _guard = Guard::acquire()?;
    unsafe { &mut *DNS.0.get() }.close(id)
}
/// Stable repeated result; validating userspace output never consumes an answer.
pub fn dns_result(id: DnsId) -> Result<(helix_core::dns_lookup::Outcome, bool), Status> {
    let _guard = Guard::acquire()?;
    let result = unsafe { &mut *DNS.0.get() }.result(id, crate::timer::nanoseconds());
    if result == Err(Status::InvalidHandle) {
        return result;
    }
    health()?;
    result
}
pub fn dns_ready(id: DnsId) -> Result<bool, Status> {
    let _guard = Guard::acquire()?;
    let result = unsafe { &mut *DNS.0.get() }.ready(id, crate::timer::nanoseconds());
    if result == Err(Status::InvalidHandle) {
        return result;
    }
    health()?;
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_health_is_distinct_from_temporary_backpressure() {
        use crate::network::NetworkError;
        assert_eq!(health_status(Ok(())), Ok(()));
        assert_eq!(
            health_status(Err(NetworkError::Busy)),
            Err(Status::WouldBlock)
        );
        assert_eq!(
            health_status(Err(NetworkError::NotFound)),
            Err(Status::NotSupported)
        );
        for error in [
            NetworkError::Dhcp,
            NetworkError::Timeout,
            NetworkError::InvalidCompletion,
        ] {
            assert_eq!(health_status(Err(error)), Err(Status::IoError));
        }
    }
    #[test]
    fn independent_policy_checks_precede_unavailable_hardware() {
        assert_eq!(create(0, 0, 4), Err(Status::AccessDenied));
        assert_eq!(create(0, 4, 0), Err(Status::AccessDenied));
    }
}
