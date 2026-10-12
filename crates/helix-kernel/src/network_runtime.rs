//! Permanent legacy virtio owner for the connected development UDP profile.
//! No pointer borrowed from a process/socket is ever submitted to the device.
use super::{clock_now, dhcp, udp, Device, NetworkError, NetworkInfo, MAX_FRAME};
use crate::network_socket::dns::{
    Broker, FIRST_PORT as DNS_PORT, REQUESTS as DNS_REQUESTS, SERVER as DNS_SERVER,
};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use helix_core::datagram::{self, Datagrams};
struct Runtime {
    device: Option<Device>,
    lease: Option<dhcp::Lease>,
    gateway_mac: [u8; 6],
    expires: u64,
    dhcp_restarts: u8,
    tx_pending: bool,
    tx_deadline: u64,
    failed: Option<NetworkError>,
}
struct Shared(UnsafeCell<Runtime>);
// Single bootstrap CPU; access requires the try-only guard. No context switch
// or wait occurs while a Rust reference to these device queues is held.
unsafe impl Sync for Shared {}
static RUNTIME: Shared = Shared(UnsafeCell::new(Runtime {
    device: None,
    lease: None,
    gateway_mac: [0; 6],
    expires: 0,
    dhcp_restarts: 0,
    tx_pending: false,
    tx_deadline: 0,
    failed: None,
}));
static LOCK: AtomicBool = AtomicBool::new(false);
struct Guard;
impl Guard {
    fn acquire() -> Result<Self, NetworkError> {
        LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| NetworkError::Busy)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}
pub(super) fn retain(device: Device, info: NetworkInfo) -> Result<(), NetworkError> {
    let _guard = Guard::acquire()?;
    let runtime = unsafe { &mut *RUNTIME.0.get() };
    if runtime.device.is_some() {
        return Err(NetworkError::Busy);
    }
    if device
        .dhcp_client
        .as_ref()
        .and_then(|client| client.lease(device.lease_at))
        != Some(info.lease)
    {
        return Err(NetworkError::Dhcp);
    }
    runtime.expires = info.lease.expires_at(device.lease_at);
    runtime.lease = Some(info.lease);
    runtime.gateway_mac = info.gateway_mac;
    runtime.device = Some(device);
    Ok(())
}
/// The actual leased interface is only available while its retained driver and
/// DHCP client report a usable lease.
pub fn datagram_address() -> Option<[u8; 4]> {
    let _guard = Guard::acquire().ok()?;
    let runtime = unsafe { &*RUNTIME.0.get() };
    runtime.health(clock_now()).ok()?;
    runtime.lease.map(|lease| lease.address)
}
/// Terminal driver errors and an expired lease dominate readiness. The device
/// and its DMA buffers remain retained; this profile supplies no recovery/reset.
pub fn datagram_health() -> Result<(), NetworkError> {
    let _guard = Guard::acquire()?;
    unsafe { &*RUNTIME.0.get() }.health(clock_now())
}
fn decode(
    frame: &[u8],
    mac: [u8; 6],
    gateway_mac: [u8; 6],
    address: [u8; 4],
) -> Option<(datagram::Endpoint, datagram::Endpoint, &[u8])> {
    let port = u16::from_be_bytes(frame.get(36..38)?.try_into().ok()?);
    if !(datagram::FIRST_PORT..datagram::FIRST_PORT + datagram::MAX_SOCKETS as u16).contains(&port)
    {
        return None;
    }
    let local = datagram::Endpoint { address, port };
    let peer = datagram::DEVELOPMENT_PEER;
    let payload = udp::decode(
        frame,
        mac,
        gateway_mac,
        udp::Endpoint {
            address: local.address,
            port: local.port,
        },
        udp::Endpoint {
            address: peer.address,
            port: peer.port,
        },
    )
    .ok()?;
    Some((local, peer, payload))
}
fn decode_dns(
    frame: &[u8],
    mac: [u8; 6],
    gateway_mac: [u8; 6],
    address: [u8; 4],
) -> Option<(u16, &[u8])> {
    let port = u16::from_be_bytes(frame.get(36..38)?.try_into().ok()?);
    if !(DNS_PORT..DNS_PORT + DNS_REQUESTS as u16).contains(&port) {
        return None;
    }
    let payload = udp::decode(
        frame,
        mac,
        gateway_mac,
        udp::Endpoint { address, port },
        udp::Endpoint {
            address: DNS_SERVER.address,
            port: DNS_SERVER.port,
        },
    )
    .ok()?;
    Some((port, payload))
}
fn transmit_completion(
    length: Option<usize>,
    now: u64,
    deadline: u64,
) -> Result<bool, NetworkError> {
    match length {
        Some(0) => Ok(true),
        Some(_) => Err(NetworkError::InvalidCompletion),
        None if now >= deadline => Err(NetworkError::Timeout),
        None => Ok(false),
    }
}
fn is_dhcp_reply(frame: &[u8]) -> bool {
    frame.len() >= 42
        && u16::from_be_bytes([frame[12], frame[13]]) == 0x0800
        && frame[23] == 17
        && u16::from_be_bytes([frame[34], frame[35]]) == 67
        && u16::from_be_bytes([frame[36], frame[37]]) == 68
}
fn accept_dhcp_reply(
    client: &mut dhcp::Client,
    current: dhcp::Lease,
    frame: &[u8],
    now: u64,
) -> Result<Option<dhcp::Lease>, NetworkError> {
    if !is_dhcp_reply(frame) {
        return Ok(None);
    }
    match client.accept(frame, now) {
        Ok(Some(renewed)) => {
            // The retained route has no ARP refresh path yet. Keep routing only
            // when the acknowledged configuration matches the active route.
            if renewed.address != current.address
                || renewed.subnet_mask != current.subnet_mask
                || renewed.gateway != current.gateway
            {
                return Err(NetworkError::DhcpRouteChanged);
            }
            Ok(Some(renewed))
        }
        Ok(None) => Ok(None),
        Err(_) if client.state() == dhcp::State::Failed => Err(NetworkError::Dhcp),
        // Invalid, stale, or unrelated DHCP traffic is ignored; it must not
        // prevent this bounded pump from servicing other sockets.
        Err(_) => Ok(None),
    }
}
/// At most one completion check, four RX completions and one TX submission.
/// DMA ownership survives socket teardown and any driver error. A failed
/// device remains claimed, with its permanent buffers retained and unreused.
pub(crate) fn datagram_pump_dns(
    store: &mut Datagrams<8>,
    dns: &mut Broker,
) -> Result<(), NetworkError> {
    let _guard = Guard::acquire()?;
    let runtime = unsafe { &mut *RUNTIME.0.get() };
    runtime.run(store, dns)
}
impl Runtime {
    fn run(&mut self, store: &mut Datagrams<8>, dns: &mut Broker) -> Result<(), NetworkError> {
        if let Some(error) = self.failed {
            if error != NetworkError::Dhcp || !self.restart_dhcp(clock_now()) {
                return Err(error);
            }
        }
        let result = unsafe { self.pump(store, dns) };
        if let Err(error) = result {
            if error == NetworkError::Dhcp && self.restart_dhcp(clock_now()) {
                return Ok(());
            }
            if !matches!(error, NetworkError::NotFound | NetworkError::Busy) {
                self.failed = Some(error);
            }
        }
        result
    }
    fn health(&self, now: u64) -> Result<(), NetworkError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let Some(device) = self.device.as_ref() else {
            return Err(NetworkError::NotFound);
        };
        let Some(lease) = self.lease else {
            return Err(NetworkError::NotFound);
        };
        if now >= self.expires
            || device
                .dhcp_client
                .as_ref()
                .and_then(|client| client.lease(now))
                != Some(lease)
        {
            return Err(NetworkError::Dhcp);
        }
        Ok(())
    }
    fn restart_dhcp(&mut self, now: u64) -> bool {
        const MAX_RESTARTS: u8 = 2;
        if self.dhcp_restarts >= MAX_RESTARTS {
            return false;
        }
        let Some(device) = self.device.as_mut() else {
            return false;
        };
        let Some(previous) = device.dhcp_client.as_ref() else {
            return false;
        };
        let state = previous.state();
        let lost = state == dhcp::State::Failed
            || (matches!(
                state,
                dhcp::State::Bound | dhcp::State::Renewing | dhcp::State::Rebinding
            ) && now >= self.expires);
        if !lost {
            return false;
        }
        let mac_tail =
            u32::from_be_bytes([device.mac[2], device.mac[3], device.mac[4], device.mac[5]]);
        let seed = unsafe { core::arch::x86_64::_rdtsc() as u32 }
            ^ now as u32
            ^ mac_tail
            ^ (self.dhcp_restarts as u32).wrapping_mul(0x9e37_79b9);
        let Ok(client) = dhcp::Client::new(device.mac, seed.max(1), now) else {
            return false;
        };
        device.dhcp_client = Some(client);
        self.dhcp_restarts += 1;
        self.failed = None;
        true
    }
    unsafe fn pump(
        &mut self,
        store: &mut Datagrams<8>,
        dns: &mut Broker,
    ) -> Result<(), NetworkError> {
        let now = clock_now();
        dns.progress(now);
        let mut lease_usable = self.health(now).is_ok();
        let device = self.device.as_mut().ok_or(NetworkError::NotFound)?;
        let mut lease = self.lease.ok_or(NetworkError::NotFound)?;
        let recovering = device.dhcp_client.as_ref().is_some_and(|client| {
            matches!(
                client.state(),
                dhcp::State::Discovering | dhcp::State::Requesting
            )
        });
        if !lease_usable && !recovering {
            return Err(NetworkError::Dhcp);
        }
        if self.tx_pending && transmit_completion(device.tx.poll()?, now, self.tx_deadline)? {
            self.tx_pending = false;
            self.tx_deadline = 0;
            let _ = super::in8(device.base + 19);
        }

        let mut frame = [0; MAX_FRAME];
        for _ in 0..4 {
            let Some(count) = device.receive(&mut frame)? else {
                break;
            };
            if is_dhcp_reply(&frame[..count]) {
                let client = device.dhcp_client.as_mut().ok_or(NetworkError::Dhcp)?;
                if let Some(renewed) = accept_dhcp_reply(client, lease, &frame[..count], now)? {
                    lease = renewed;
                    device.lease_at = now;
                    self.expires = renewed.expires_at(now);
                    self.lease = Some(renewed);
                    lease_usable = true;
                }
            } else if lease_usable {
                if let Some((port, payload)) =
                    decode_dns(&frame[..count], device.mac, self.gateway_mac, lease.address)
                {
                    dns.receive(port, DNS_SERVER, payload, now);
                } else if let Some((local, peer, payload)) =
                    decode(&frame[..count], device.mac, self.gateway_mac, lease.address)
                {
                    // A full destination queue drops this datagram; it never
                    // changes already queued payloads or fabricates readiness.
                    let _ = store.deliver(local, peer, payload);
                }
            }
        }
        if !self.tx_pending {
            let mut request = [0; MAX_FRAME];
            let client = device.dhcp_client.as_mut().ok_or(NetworkError::Dhcp)?;
            if let Some(count) = client
                .request(now, &mut request)
                .map_err(|_| NetworkError::Dhcp)?
            {
                device.submit_transmit(&request[..count])?;
                self.tx_pending = true;
                self.tx_deadline = now.saturating_add(2_000_000_000);
                return Ok(());
            }
        }
        if lease_usable && !self.tx_pending {
            let mut query = [0; helix_core::dns::PACKET_BYTES];
            if let Some((port, length)) = dns.next_query(now, &mut query) {
                let count = udp::encode(
                    device.mac,
                    self.gateway_mac,
                    udp::Endpoint {
                        address: lease.address,
                        port,
                    },
                    udp::Endpoint {
                        address: DNS_SERVER.address,
                        port: DNS_SERVER.port,
                    },
                    &query[..length],
                    &mut frame,
                )
                .map_err(|_| NetworkError::Packet)?;
                device.submit_transmit(&frame[..count])?;
                self.tx_pending = true;
                self.tx_deadline = now.saturating_add(2_000_000_000);
            }
        }
        if lease_usable && !self.tx_pending {
            let mut payload = [0; datagram::MAX_PAYLOAD];
            match store.next_transmit(&mut payload) {
                Ok(packet) => {
                    let count = udp::encode(
                        device.mac,
                        self.gateway_mac,
                        udp::Endpoint {
                            address: packet.local.address,
                            port: packet.local.port,
                        },
                        udp::Endpoint {
                            address: packet.peer.address,
                            port: packet.peer.port,
                        },
                        &payload[..packet.length],
                        &mut frame,
                    )
                    .map_err(|_| NetworkError::Packet)?;
                    device.submit_transmit(&frame[..count])?;
                    self.tx_pending = true;
                    self.tx_deadline = now.saturating_add(2_000_000_000);
                    // TX_BUFFER now owns the bytes until device completion,
                    // even if the socket and its queues close immediately.
                    store
                        .consume_transmit(packet.socket, packet.ticket)
                        .map_err(|_| NetworkError::Packet)?;
                }
                Err(datagram::Error::WouldBlock) => {}
                Err(_) => return Err(NetworkError::Packet),
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const CLIENT_MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const SERVER_MAC: [u8; 6] = [0x52, 0x55, 10, 0, 2, 2];
    const XID: u32 = 0x4e58_5431;

    fn checksum(bytes: &[u8]) -> u16 {
        let mut sum = 0u32;
        for pair in bytes.chunks(2) {
            sum += ((pair[0] as u32) << 8) | pair.get(1).copied().unwrap_or(0) as u32;
        }
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    fn udp_checksum(source: [u8; 4], destination: [u8; 4], udp: &[u8]) -> u16 {
        let mut bytes = std::vec::Vec::with_capacity(12 + udp.len());
        bytes.extend_from_slice(&source);
        bytes.extend_from_slice(&destination);
        bytes.extend_from_slice(&[0, 17]);
        bytes.extend_from_slice(&(udp.len() as u16).to_be_bytes());
        bytes.extend_from_slice(udp);
        checksum(&bytes)
    }

    fn dhcp_reply(kind: u8, xid: u32, lease_seconds: u32, gateway: [u8; 4]) -> [u8; 342] {
        let mut frame = [0; 342];
        frame[..6].copy_from_slice(&CLIENT_MAC);
        frame[6..12].copy_from_slice(&SERVER_MAC);
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14] = 0x45;
        frame[16..18].copy_from_slice(&328u16.to_be_bytes());
        frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
        frame[22] = 64;
        frame[23] = 17;
        frame[26..30].copy_from_slice(&[10, 0, 2, 2]);
        frame[30..34].copy_from_slice(&[10, 0, 2, 15]);
        frame[34..36].copy_from_slice(&67u16.to_be_bytes());
        frame[36..38].copy_from_slice(&68u16.to_be_bytes());
        frame[38..40].copy_from_slice(&308u16.to_be_bytes());
        let bootp = &mut frame[42..];
        bootp[0] = 2;
        bootp[1] = 1;
        bootp[2] = 6;
        bootp[4..8].copy_from_slice(&xid.to_be_bytes());
        bootp[16..20].copy_from_slice(&[10, 0, 2, 15]);
        bootp[28..34].copy_from_slice(&CLIENT_MAC);
        bootp[236..240].copy_from_slice(&[99, 130, 83, 99]);
        let mut options = [0; 46];
        if kind == 6 {
            options[..10].copy_from_slice(&[53, 1, 6, 54, 4, 10, 0, 2, 2, 255]);
        } else {
            options[0..9].copy_from_slice(&[53, 1, kind, 54, 4, 10, 0, 2, 2]);
            options[9..15].copy_from_slice(&[1, 4, 255, 255, 255, 0]);
            options[15..21]
                .copy_from_slice(&[3, 4, gateway[0], gateway[1], gateway[2], gateway[3]]);
            options[21..27].copy_from_slice(&[6, 4, 10, 0, 2, 3]);
            options[27..33].copy_from_slice(&[51, 4, 0, 0, 0, 0]);
            options[29..33].copy_from_slice(&lease_seconds.to_be_bytes());
            options[33..39].copy_from_slice(&[58, 4, 0, 0, 0, 0]);
            options[35..39].copy_from_slice(&(lease_seconds / 2).to_be_bytes());
            options[39..45].copy_from_slice(&[59, 4, 0, 0, 0, 0]);
            options[41..45].copy_from_slice(&(lease_seconds * 7 / 8).to_be_bytes());
            options[45] = 255;
        }
        bootp[240..240 + options.len()].copy_from_slice(&options);
        let ip_checksum = checksum(&frame[14..34]);
        frame[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
        let udp = udp_checksum([10, 0, 2, 2], [10, 0, 2, 15], &frame[34..]);
        frame[40..42].copy_from_slice(&(if udp == 0 { 0xffff } else { udp }).to_be_bytes());
        frame
    }

    fn bound_client() -> (dhcp::Client, dhcp::Lease) {
        let mut client = dhcp::Client::new(CLIENT_MAC, XID, 0).unwrap();
        let mut outgoing = [0; MAX_FRAME];
        client.request(0, &mut outgoing).unwrap();
        let offer = dhcp_reply(2, XID, 40, [10, 0, 2, 2]);
        client.accept(&offer, 1).unwrap();
        client.request(1, &mut outgoing).unwrap();
        let ack = dhcp_reply(5, XID, 40, [10, 0, 2, 2]);
        let lease = client.accept(&ack, 2).unwrap().unwrap();
        (client, lease)
    }

    #[test]
    fn dns_reserved_ports_and_exact_resolver_tuple_are_independent_from_native_udp() {
        let mac = [2, 1, 2, 3, 4, 5];
        let gateway_mac = [2, 9, 8, 7, 6, 5];
        let address = [10, 0, 2, 15];
        let mut frame = [0; MAX_FRAME];
        let length = udp::encode(
            gateway_mac,
            mac,
            udp::Endpoint {
                address: DNS_SERVER.address,
                port: DNS_SERVER.port,
            },
            udp::Endpoint {
                address,
                port: DNS_PORT,
            },
            b"query",
            &mut frame,
        )
        .unwrap();
        assert_eq!(
            decode_dns(&frame[..length], mac, gateway_mac, address),
            Some((DNS_PORT, &b"query"[..]))
        );
        assert!(decode(&frame[..length], mac, gateway_mac, address).is_none());
        for cut in 0..length {
            assert!(decode_dns(&frame[..cut], mac, gateway_mac, address).is_none());
        }
        for (peer, port) in [
            (
                udp::Endpoint {
                    address: DNS_SERVER.address,
                    port: DNS_SERVER.port + 1,
                },
                DNS_PORT,
            ),
            (
                udp::Endpoint {
                    address: [10, 0, 2, 3],
                    port: DNS_SERVER.port,
                },
                DNS_PORT,
            ),
            (
                udp::Endpoint {
                    address: DNS_SERVER.address,
                    port: DNS_SERVER.port,
                },
                datagram::FIRST_PORT,
            ),
            (
                udp::Endpoint {
                    address: DNS_SERVER.address,
                    port: DNS_SERVER.port,
                },
                DNS_PORT + DNS_REQUESTS as u16,
            ),
        ] {
            let length = udp::encode(
                gateway_mac,
                mac,
                peer,
                udp::Endpoint { address, port },
                b"query",
                &mut frame,
            )
            .unwrap();
            assert!(decode_dns(&frame[..length], mac, gateway_mac, address).is_none());
        }
    }
    #[test]
    fn transmit_completion_wins_at_deadline_and_timeout_preserves_dma_owner() {
        assert_eq!(transmit_completion(None, 99, 100), Ok(false));
        assert_eq!(
            transmit_completion(None, 100, 100),
            Err(NetworkError::Timeout)
        );
        assert_eq!(transmit_completion(Some(0), 100, 100), Ok(true));
        assert_eq!(
            transmit_completion(Some(1), 100, 100),
            Err(NetworkError::InvalidCompletion)
        );
        let memory = super::super::Dma(UnsafeCell::new([0u8; super::super::QUEUE_BYTES]));
        let tx = super::super::Queue {
            memory: memory.0.get().cast(),
            count: 256,
            avail: 4096,
            used: 8192,
            submitted: 1,
            completed: 0,
        };
        let rx = super::super::Queue {
            memory: core::ptr::null_mut(),
            count: 1,
            avail: 0,
            used: 0,
            submitted: 1,
            completed: 0,
        };
        let (client, lease) = bound_client();
        let mut runtime = Runtime {
            device: Some(Device {
                base: 0,
                mac: CLIENT_MAC,
                rx,
                tx,
                lease_at: 2,
                dhcp_client: Some(client),
            }),
            lease: Some(lease),
            gateway_mac: [0; 6],
            expires: lease.expires_at(2),
            dhcp_restarts: 0,
            tx_pending: true,
            tx_deadline: 0,
            failed: None,
        };
        let mut sockets = Datagrams::<8>::new();
        // Host clock0 reaches deadline0. Timeout returns before RX polling or
        // any port I/O; the outstanding descriptor and owner remain intact.
        assert_eq!(
            runtime.run(&mut sockets, &mut Broker::new()),
            Err(NetworkError::Timeout)
        );
        assert_eq!(runtime.health(0), Err(NetworkError::Timeout));
        assert_eq!(
            runtime.run(&mut sockets, &mut Broker::new()),
            Err(NetworkError::Timeout)
        );
        assert!(runtime.tx_pending);
        assert_eq!(runtime.tx_deadline, 0);
        let device = runtime.device.as_ref().unwrap();
        assert_eq!(device.tx.submitted, 1);
        assert_eq!(device.tx.completed, 0);
        assert!(runtime.lease.is_some());
    }

    #[test]
    fn runtime_renews_through_retained_client_and_refreshes_expiry_only_for_safe_ack() {
        let (mut client, current) = bound_client();
        let renew_at = 20_000_000_002;
        let mut outgoing = [0; MAX_FRAME];
        let count = client.request(renew_at, &mut outgoing).unwrap().unwrap();
        assert_eq!(count, 342);
        assert_eq!(client.state(), dhcp::State::Renewing);
        assert_eq!(&outgoing[0..6], &SERVER_MAC);
        assert_eq!(&outgoing[26..30], &current.address);
        assert_eq!(&outgoing[30..34], &current.server);
        let renewal_xid = u32::from_be_bytes(outgoing[46..50].try_into().unwrap());

        assert_eq!(
            accept_dhcp_reply(&mut client, current, &[0; 64], renew_at + 1),
            Ok(None)
        );
        let ack = dhcp_reply(5, renewal_xid, 60, [10, 0, 2, 2]);
        let renewed = accept_dhcp_reply(&mut client, current, &ack, renew_at + 1)
            .unwrap()
            .unwrap();
        assert_eq!(renewed.address, current.address);
        assert_eq!(renewed.lease_seconds, 60);
        assert_eq!(renewed.expires_at(renew_at + 1), renew_at + 60_000_000_001);
        assert_eq!(client.state(), dhcp::State::Bound);

        let (mut client, current) = bound_client();
        let count = client.request(renew_at, &mut outgoing).unwrap().unwrap();
        let renewal_xid = u32::from_be_bytes(outgoing[46..50].try_into().unwrap());
        let changed_route = dhcp_reply(5, renewal_xid, 60, [10, 0, 2, 3]);
        assert_eq!(
            accept_dhcp_reply(&mut client, current, &changed_route, renew_at + 1),
            Err(NetworkError::DhcpRouteChanged)
        );
        assert_eq!(client.state(), dhcp::State::Bound);
        assert_eq!(count, 342);
    }

    #[test]
    fn renewal_nak_becomes_terminal_and_malformed_dhcp_does_not_poison_runtime() {
        let (mut client, current) = bound_client();
        let renew_at = 20_000_000_002;
        let mut outgoing = [0; MAX_FRAME];
        client.request(renew_at, &mut outgoing).unwrap();
        let renewal_xid = u32::from_be_bytes(outgoing[46..50].try_into().unwrap());
        let mut malformed = dhcp_reply(5, renewal_xid, 60, [10, 0, 2, 2]);
        malformed[24] ^= 0x80;
        assert_eq!(
            accept_dhcp_reply(&mut client, current, &malformed, renew_at + 1),
            Ok(None)
        );
        assert_eq!(client.state(), dhcp::State::Renewing);
        let nak = dhcp_reply(6, renewal_xid, 0, [10, 0, 2, 2]);
        assert_eq!(
            accept_dhcp_reply(&mut client, current, &nak, renew_at + 1),
            Err(NetworkError::Dhcp)
        );
        assert_eq!(client.state(), dhcp::State::Failed);
    }

    #[test]
    fn lease_loss_restarts_discovery_at_most_twice_and_keeps_the_old_route_fail_closed() {
        fn exhaust_discovery(client: &mut dhcp::Client, now: &mut u64) {
            let mut out = [0; MAX_FRAME];
            for _ in 0..3 {
                assert!(client.request(*now, &mut out).unwrap().is_some());
                *now += 1_000_000_000;
            }
            assert_eq!(client.request(*now, &mut out), Err(dhcp::Error::Timeout));
        }
        fn queue() -> super::super::Queue {
            super::super::Queue {
                memory: core::ptr::null_mut(),
                count: 1,
                avail: 0,
                used: 0,
                submitted: 0,
                completed: 0,
            }
        }
        let (client, lease) = bound_client();
        let expires = lease.expires_at(2);
        let mut runtime = Runtime {
            device: Some(Device {
                base: 0,
                mac: CLIENT_MAC,
                rx: queue(),
                tx: queue(),
                lease_at: 2,
                dhcp_client: Some(client),
            }),
            lease: Some(lease),
            gateway_mac: SERVER_MAC,
            expires,
            dhcp_restarts: 0,
            tx_pending: false,
            tx_deadline: 0,
            failed: None,
        };
        assert_eq!(runtime.health(expires), Err(NetworkError::Dhcp));
        assert!(runtime.restart_dhcp(expires));
        assert_eq!(runtime.dhcp_restarts, 1);
        assert_eq!(
            runtime
                .device
                .as_ref()
                .unwrap()
                .dhcp_client
                .as_ref()
                .unwrap()
                .state(),
            dhcp::State::Discovering
        );
        // A client already reacquiring is not repeatedly replaced on every
        // pump while the retained interface remains without a valid lease.
        assert!(!runtime.restart_dhcp(expires));

        let mut now = expires;
        exhaust_discovery(
            runtime
                .device
                .as_mut()
                .unwrap()
                .dhcp_client
                .as_mut()
                .unwrap(),
            &mut now,
        );
        assert!(runtime.restart_dhcp(now));
        assert_eq!(runtime.dhcp_restarts, 2);
        exhaust_discovery(
            runtime
                .device
                .as_mut()
                .unwrap()
                .dhcp_client
                .as_mut()
                .unwrap(),
            &mut now,
        );
        assert!(!runtime.restart_dhcp(now));
        assert_eq!(runtime.lease, Some(lease));
        assert_eq!(runtime.health(now), Err(NetworkError::Dhcp));
    }

    #[test]
    fn packet_stress_preserves_bounded_payloads_and_exact_routes() {
        let mac = [2, 1, 2, 3, 4, 5];
        let gateway_mac = [2, 7, 8, 9, 10, 11];
        let local = udp::Endpoint {
            address: [10, 0, 2, 15],
            port: datagram::FIRST_PORT,
        };
        let peer = udp::Endpoint {
            address: [10, 0, 2, 2],
            port: 39097,
        };
        let mut valid = [0; MAX_FRAME];
        let length = udp::encode(
            gateway_mac,
            mac,
            peer,
            local,
            &[0xa5; datagram::MAX_PAYLOAD],
            &mut valid,
        )
        .unwrap();
        assert_eq!(length, MAX_FRAME);
        assert_eq!(
            decode(&valid, mac, gateway_mac, local.address)
                .unwrap()
                .2
                .len(),
            datagram::MAX_PAYLOAD
        );
        let mut seed = 0x5eedu32;
        for round in 0..4096 {
            let mut candidate = valid;
            for _ in 0..1 + round % 5 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                candidate[seed as usize % MAX_FRAME] ^= 1 << (seed >> 24 & 7);
            }
            let length = if round % 2 == 0 {
                MAX_FRAME
            } else {
                round % (MAX_FRAME + 1)
            };
            if let Some((actual_local, actual_peer, payload)) =
                decode(&candidate[..length], mac, gateway_mac, local.address)
            {
                assert_eq!(actual_local.address, local.address);
                assert!(
                    (datagram::FIRST_PORT..datagram::FIRST_PORT + 8).contains(&actual_local.port)
                );
                assert_eq!(actual_peer, datagram::DEVELOPMENT_PEER);
                assert!(payload.len() <= datagram::MAX_PAYLOAD);
            }
        }
    }

    #[test]
    fn retained_driver_failure_and_expired_lease_are_terminal() {
        fn queue() -> super::super::Queue {
            super::super::Queue {
                memory: core::ptr::null_mut(),
                count: 1,
                avail: 0,
                used: 0,
                submitted: 0,
                completed: 0,
            }
        }
        let (client, lease) = bound_client();
        let device = Device {
            base: 0,
            mac: CLIENT_MAC,
            rx: queue(),
            tx: queue(),
            lease_at: 2,
            dhcp_client: Some(client),
        };
        let mut runtime = Runtime {
            device: Some(device),
            lease: Some(lease),
            gateway_mac: [0; 6],
            expires: lease.expires_at(2),
            dhcp_restarts: 0,
            tx_pending: false,
            tx_deadline: 0,
            failed: None,
        };
        assert_eq!(runtime.health(lease.expires_at(2) - 1), Ok(()));
        assert_eq!(runtime.health(lease.expires_at(2)), Err(NetworkError::Dhcp));
        runtime.failed = Some(NetworkError::InvalidCompletion);
        assert_eq!(runtime.health(0), Err(NetworkError::InvalidCompletion));
        runtime.device = None;
        assert_eq!(runtime.health(0), Err(NetworkError::InvalidCompletion));
        runtime.failed = None;
        assert_eq!(runtime.health(0), Err(NetworkError::NotFound));
    }
    #[test]
    fn exact_peer_udp_checksums_and_local_socket_range_are_enforced() {
        let local_mac = [2, 1, 2, 3, 4, 5];
        let peer_mac = [2, 7, 8, 9, 10, 11];
        let local = udp::Endpoint {
            address: [10, 0, 2, 15],
            port: datagram::FIRST_PORT,
        };
        let peer = udp::Endpoint {
            address: [10, 0, 2, 2],
            port: 39097,
        };
        let mut frame = [0; MAX_FRAME];
        let n = udp::encode(peer_mac, local_mac, peer, local, b"nonce-pong", &mut frame).unwrap();
        assert_eq!(
            decode(&frame[..n], local_mac, peer_mac, local.address)
                .unwrap()
                .2,
            b"nonce-pong"
        );
        for index in [0, 6, 14, 24, 26, 30, 34, 36, 40, 42] {
            let mut damaged = frame;
            damaged[index] ^= 1;
            assert!(
                decode(&damaged[..n], local_mac, peer_mac, local.address).is_none(),
                "offset{index}"
            );
        }
        for length in 0..n {
            assert!(decode(&frame[..length], local_mac, peer_mac, local.address).is_none());
        }
        let invalid_port = udp::Endpoint {
            port: datagram::FIRST_PORT - 1,
            ..local
        };
        let n = udp::encode(peer_mac, local_mac, peer, invalid_port, b"", &mut frame).unwrap();
        assert!(decode(&frame[..n], local_mac, peer_mac, local.address).is_none());
    }
}
